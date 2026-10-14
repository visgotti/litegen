//! Materialise the mesh conversion matrix on disk for render verification.
//!
//! ```text
//! cargo run --example mesh_matrix -- <out_dir> [--fixtures a,b] [--formats glb,obj] [--import <dir>]
//! ```
//!
//! For every fixture (see `litegen::mesh::fixtures`) and every supported
//! container this writes:
//!
//! - `<fixture>/reference.glb` — the full scene, the picture everything is
//!   compared against;
//! - `<fixture>/src/<fmt>/…` — the scene written as `<fmt>` by our writer;
//! - `<fixture>/conv/<from>__<to>/…` — `src/<from>` read back by our reader and
//!   written as `<to>`: one directory per direction of conversion;
//! - `<fixture>/expected/<hash>.glb` — the scene as the target writer encodes
//!   it after the source writer already has (`to.prepare(from.prepare(scene))`),
//!   which is what a correct conversion must look like;
//! - `manifest.json` — every case, its expected picture, the camera, the sample
//!   times, and the losses each writer reported.
//!
//! `scripts/mesh-verify/verify.mjs` then renders every file with an independent
//! loader and compares. `--import <dir>` adds third-party files (each read with
//! the format its extension names) as extra rows, so readers are exercised on
//! bytes our own writers never produced.

use litegen::mesh::{
    self, fixtures, package::Sidecars, Capabilities, MaterialSupport, MeshFormat, Package, Scene,
    WriteOptions,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn caps_key(c: &Capabilities) -> String {
    let m = match c.materials {
        MaterialSupport::None => "0",
        MaterialSupport::VertexColor => "v",
        MaterialSupport::ColorOnly => "c",
        MaterialSupport::Phong => "p",
        MaterialSupport::Pbr => "r",
    };
    format!(
        "h{}n{}u{}v{}m{}t{}s{}x{}a{}g{}",
        c.hierarchy as u8,
        c.normals as u8,
        c.uv_sets,
        c.vertex_colors as u8,
        m,
        c.textures as u8,
        c.skinning as u8,
        c.morph_targets as u8,
        c.animation as u8,
        c.multiple_meshes as u8
    )
}

/// FNV-1a, to name expected files by content.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn caps_json(c: &Capabilities) -> Value {
    json!({
        "hierarchy": c.hierarchy, "normals": c.normals, "uv_sets": c.uv_sets,
        "vertex_colors": c.vertex_colors,
        "materials": format!("{:?}", c.materials), "textures": c.textures,
        "skinning": c.skinning, "morph_targets": c.morph_targets,
        "animation": c.animation, "multiple_meshes": c.multiple_meshes,
    })
}

fn write_package(dir: &Path, pkg: &Package) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut names = Vec::new();
    for f in &pkg.files {
        let p = dir.join(&f.path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, &f.bytes)?;
        names.push(f.path.clone());
    }
    Ok(names)
}

/// Sidecars backed by a directory, for reading a package back.
struct DirSidecars(PathBuf);

impl Sidecars for DirSidecars {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

fn camera(scene: &Scene) -> Value {
    let (lo, hi) = fixtures::bounds_over_animation(scene).unwrap_or(([-1.0; 3], [1.0; 3]));
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = (0..3).map(|i| (hi[i] - lo[i]).powi(2)).sum::<f32>().sqrt() / 2.0;
    let r = r.max(1e-3);
    let fov: f32 = 40.0;
    let dist = r / (fov.to_radians() / 2.0).sin() * 1.02;
    let d = [1.0f32, 0.7, 1.5];
    let dl = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let pos = [c[0] + d[0] / dl * dist, c[1] + d[1] / dl * dist, c[2] + d[2] / dl * dist];
    json!({
        "target": c, "position": pos, "fov": fov,
        "near": (dist - r * 1.5).max(dist * 0.01), "far": dist + r * 1.5
    })
}

struct Case {
    expected_key: Option<String>,
    /// Why no expected picture exists, when none does: a case whose
    /// conversion is refused is right to be when the expected scene cannot be
    /// written either (a model with no area at rest has nothing to print).
    expected_error: Option<String>,
    fixture: String,
    from: String,
    to: String,
    dir: PathBuf,
    files: Vec<String>,
    caps: Capabilities,
    losses: Vec<String>,
    warnings: Vec<String>,
    error: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first().map(PathBuf::from) else {
        eprintln!("usage: mesh_matrix <out_dir> [--fixtures a,b] [--formats glb,obj] [--import <dir>]");
        std::process::exit(2);
    };
    let opt = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    };
    let only_fixtures: Option<Vec<String>> = opt("--fixtures").map(|s| s.split(',').map(str::to_string).collect());
    let formats: Vec<MeshFormat> = match opt("--formats") {
        Some(s) => s.split(',').filter_map(MeshFormat::parse).collect(),
        None => MeshFormat::ALL.to_vec(),
    };
    let package_mode = WriteOptions { self_contained: false };

    let mut sources: Vec<(String, Scene)> = fixtures::all()
        .into_iter()
        .filter(|(n, _)| only_fixtures.as_ref().is_none_or(|o| o.iter().any(|x| x == n)))
        .map(|(n, s)| (n.to_string(), s))
        .collect();

    // Third-party inputs: each becomes a fixture read by OUR reader. Its own
    // native rendering is the harness's independent reference for it.
    let mut imported: Vec<(String, String, PathBuf)> = Vec::new();
    if let Some(dir) = opt("--import") {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
            .expect("import dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file())
            .collect();
        entries.sort();
        for p in entries {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
            let Some(fmt) = MeshFormat::parse(&ext) else { continue };
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("import").to_string();
            let bytes = std::fs::read(&p).expect("read import");
            let side = DirSidecars(p.parent().unwrap().to_path_buf());
            match mesh::read_scene(&bytes, fmt, &side) {
                Ok(r) => {
                    let name = format!("tp_{}_{}", stem, fmt.as_str());
                    for w in &r.warnings {
                        eprintln!("{name}: warning: {w}");
                    }
                    imported.push((name.clone(), ext.clone(), p.clone()));
                    sources.push((name, r.scene));
                }
                Err(e) => eprintln!("import {}: {e}", p.display()),
            }
        }
    }

    let mut manifest_fixtures = serde_json::Map::new();
    let mut cases: Vec<Case> = Vec::new();

    for (name, scene) in &sources {
        let fdir = out.join(name);
        std::fs::create_dir_all(&fdir).unwrap();
        let reference = mesh::write_scene(scene, MeshFormat::Glb, package_mode).expect("reference glb");
        std::fs::write(fdir.join("reference.glb"), &reference.package.primary().bytes).unwrap();
        // Expected pictures, deduplicated by content: the scene as the target
        // writer will encode it, after the source writer already has.
        let mut expected = serde_json::Map::new();
        let mut want_expected = |prepared: Result<Scene, mesh::MeshError>| -> Result<String, String> {
            let d = match prepared {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{name}: expected scene failed: {e}");
                    return Err(e.to_string());
                }
            };
            let bytes = match mesh::write_scene(&d, MeshFormat::Glb, package_mode) {
                Ok(w) => w.package.files[0].bytes.clone(),
                Err(e) => {
                    eprintln!("{name}: expected glb failed: {e}");
                    return Err(e.to_string());
                }
            };
            let key = format!("{:016x}", fnv1a(&bytes));
            if !expected.contains_key(&key) {
                let path = format!("{name}/expected/{key}.glb");
                std::fs::create_dir_all(fdir.join("expected")).unwrap();
                std::fs::write(out.join(&path), &bytes).unwrap();
                expected.insert(key.clone(), Value::from(path));
            }
            Ok(key)
        };

        // Direct writes: the scene into every container.
        for &to in &formats {
            let dir = fdir.join("src").join(to.as_str());
            let caps = to.capabilities(package_mode);
            let expected = want_expected(to.prepare(scene, package_mode));
            let mut case = Case {
                expected_key: expected.clone().ok(),
                expected_error: expected.err(),
                fixture: name.clone(),
                from: "scene".into(),
                to: to.as_str().into(),
                dir: dir.clone(),
                files: Vec::new(),
                caps,
                losses: Vec::new(),
                warnings: Vec::new(),
                error: None,
            };
            match mesh::write_scene(scene, to, package_mode) {
                Ok(w) => {
                    case.files = write_package(&dir, &w.package).unwrap();
                    case.losses = w.losses.iter().map(|l| l.to_string()).collect();
                }
                Err(e) => case.error = Some(e.to_string()),
            }
            cases.push(case);
        }

        // Every direction: read src/<from> with our reader, write <to>.
        for &from in &formats {
            let sdir = fdir.join("src").join(from.as_str());
            let primary = cases
                .iter()
                .find(|c| c.fixture == *name && c.from == "scene" && c.to == from.as_str())
                .and_then(|c| c.files.first().cloned());
            for &to in &formats {
                let dir = fdir.join("conv").join(format!("{}__{}", from.as_str(), to.as_str()));
                let caps = from.capabilities(package_mode).intersect(to.capabilities(package_mode));
                let expected = want_expected(
                    from.prepare(scene, package_mode).and_then(|s| to.prepare(&s, package_mode)),
                );
                let mut case = Case {
                    expected_key: expected.clone().ok(),
                    expected_error: expected.err(),
                    fixture: name.clone(),
                    from: from.as_str().into(),
                    to: to.as_str().into(),
                    dir: dir.clone(),
                    files: Vec::new(),
                    caps,
                    losses: Vec::new(),
                    warnings: Vec::new(),
                    error: None,
                };
                let Some(primary) = &primary else {
                    case.error = Some(format!("no {} source was written", from.as_str()));
                    cases.push(case);
                    continue;
                };
                let bytes = std::fs::read(sdir.join(primary)).unwrap();
                let side = DirSidecars(sdir.clone());
                match mesh::read_scene(&bytes, from, &side) {
                    Err(e) => case.error = Some(format!("read {}: {e}", from.as_str())),
                    Ok(r) => {
                        case.warnings = r.warnings;
                        match mesh::write_scene(&r.scene, to, package_mode) {
                            Ok(w) => {
                                case.files = write_package(&dir, &w.package).unwrap();
                                case.losses = w.losses.iter().map(|l| l.to_string()).collect();
                            }
                            Err(e) => case.error = Some(format!("write {}: {e}", to.as_str())),
                        }
                    }
                }
                cases.push(case);
            }
        }

        // The original, with every file beside it: a .gltf or .obj names its
        // sidecars relative to itself, and its own loader must find them.
        let native = imported.iter().find(|(n, _, _)| n == name).map(|(_, ext, p)| {
            let ndir = fdir.join("native");
            std::fs::create_dir_all(&ndir).unwrap();
            let src_dir = p.parent().unwrap();
            for e in std::fs::read_dir(src_dir).unwrap().flatten() {
                let ep = e.path();
                if ep.is_file() {
                    std::fs::copy(&ep, ndir.join(e.file_name())).unwrap();
                } else if ep.is_dir() {
                    // One level of subdirectories (textures/, …) is enough
                    // for every sample set this is used with.
                    let sub = ndir.join(e.file_name());
                    std::fs::create_dir_all(&sub).unwrap();
                    for f in std::fs::read_dir(&ep).unwrap().flatten() {
                        if f.path().is_file() {
                            std::fs::copy(f.path(), sub.join(f.file_name())).unwrap();
                        }
                    }
                }
            }
            let file = p.file_name().unwrap().to_string_lossy().into_owned();
            json!({ "path": format!("{name}/native/{file}"), "format": ext })
        });
        manifest_fixtures.insert(
            name.clone(),
            json!({
                "camera": camera(scene),
                "times": fixtures::sample_times(scene).iter().map(|t| (t * 1000.0).round() / 1000.0).collect::<Vec<f32>>(),
                "reference": format!("{name}/reference.glb"),
                "expected": Value::Object(expected),
                "native": native,
                "triangles": scene.triangle_count(),
                "animations": scene.animations.iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
            }),
        );
    }

    let cases_json: Vec<Value> = cases
        .iter()
        .map(|c| {
            let rel = c.dir.strip_prefix(&out).unwrap_or(&c.dir).to_string_lossy().replace('\\', "/");
            json!({
                "fixture": c.fixture, "from": c.from, "to": c.to,
                "dir": rel,
                "primary": c.files.first().map(|f| format!("{rel}/{f}")),
                "files": c.files,
                "caps_key": caps_key(&c.caps),
                "expected": c.expected_key,
                "expected_error": c.expected_error,
                "caps": caps_json(&c.caps),
                "losses": c.losses, "warnings": c.warnings, "error": c.error,
            })
        })
        .collect();
    let errors = cases.iter().filter(|c| c.error.is_some() && c.expected_error.is_none()).count();
    let refused = cases.iter().filter(|c| c.error.is_some() && c.expected_error.is_some()).count();
    let manifest = json!({
        "formats": formats.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "fixtures": Value::Object(manifest_fixtures),
        "cases": cases_json,
    });
    std::fs::write(out.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    println!(
        "{} cases, {} conversion errors, {} refused as the expected scene is -> {}",
        cases.len(),
        errors,
        refused,
        out.join("manifest.json").display()
    );
    for c in cases.iter().filter(|c| c.error.is_some()) {
        let why = c.expected_error.as_deref().map(|e| format!(" (expected: {e})")).unwrap_or_default();
        println!("  {} {} -> {}: {}{why}", c.fixture, c.from, c.to, c.error.as_deref().unwrap_or(""));
    }
}
