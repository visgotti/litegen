//! Materialise the mesh conversion matrix on disk for render verification.
//!
//! ```text
//! cargo run --example mesh_matrix -- <out_dir> [--fixtures a,b] [--formats glb,obj] [--import <dir>] [--transcode-textures] [--dae-keep] [--usd-variants]
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
//! bytes our own writers never produced. `--transcode-textures` (imports only)
//! runs each imported scene through `mesh::normalize::transcode_textures`
//! right after it is read, as delivery and the converter do: a KTX2 or WebP
//! texture becomes PNG, so the matrix holds litegen's production output and not
//! the library's own writers' (which keep a KTX2 as bytes in glTF). Without the
//! flag nothing changes.
//!
//! `--dae-keep` (mesh-gaps item 8, Ruling DM15) adds, when `dae` is among the
//! formats, every direction into Collada once more, written with
//! `DaeMorphs::Keep` and named `dae-keep`: `src/dae-keep/`,
//! `src_single/dae-keep/` and `conv/<from>__dae-keep/`, each case with `to:
//! "dae-keep"` in the manifest and an expected scene of `dae::prepare` under
//! `Keep`. `verify.mjs` reads such a case as Collada for every judge, except
//! that three.js and assimp are expected to refuse a file that holds a
//! `<morph>`. Without the flag the output, case counts included, is unchanged.
//! Given without `dae` in `--formats` it adds nothing, and says so on stderr.
//!
//! `--usd-variants` (mesh-gaps item 6) adds, when `usdz` is among the formats,
//! every direction into USDZ three times more: `usdz-all` (a USDA root) and
//! `usdz-crate` (`usdz_root: usdc`, a USDC root), both with `usd_clips: all` (the
//! default for a USDZ is `first`, so a multi-clip USDZ is only written by
//! asking), and `usdz-crate-first` (a USDC root with the default single clip,
//! the plain crate-rooted USDZ a device check needs).
//! They are named in the manifest (`to: "usdz-all"`, `"usdz-crate"`,
//! `"usdz-crate-first"`) and written
//! to `src/<name>/` and `conv/<from>__<name>/`, each with an expected scene of
//! `usdz::prepare` under the same options. `verify.mjs` reads such a case as USDZ
//! for every judge. Without the flag the output is unchanged. Given without `usdz`
//! in `--formats` it adds nothing, and says so on stderr.
//!
//! The zip check: every case's package also goes through
//! `mesh::zip::write_package`, as a package download ships it, and is unpacked
//! by Python's `zipfile` (an independent reader) into a scratch folder that
//! must equal the case folder byte for byte; any difference fails the case.
//! The zips stay in `<out_dir>/_zips/` for `unzip -t` and `zipfile -t`, with
//! `_utf8_names.zip`, a control package whose entry names are not ASCII.

use litegen::mesh::{
    self, fixtures, package::Sidecars, Capabilities, DaeMorphs, MaterialSupport, MeshFormat, Package, PackageFile,
    Scene, UsdClips, UsdzRoot, WriteOptions,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
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

/// Every file under `dir`, by its `/`-separated path relative to `dir`.
fn files_under(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p)?);
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(out)
}

/// The zip check: `pkg` through `mesh::zip::write_package`, unpacked by
/// `python3 -I -m zipfile -e`, must be `case_dir` byte for byte. The zip is
/// kept as `<out>/_zips/<id>.zip`; the unpacked copy is removed.
fn zip_check(out: &Path, id: &str, pkg: &Package, case_dir: &Path) -> Result<(), String> {
    let zips = out.join("_zips");
    std::fs::create_dir_all(&zips).map_err(|e| e.to_string())?;
    let zip = mesh::zip::write_package(pkg).map_err(|e| format!("write_package: {e}"))?;
    let path = zips.join(format!("{id}.zip"));
    std::fs::write(&path, &zip).map_err(|e| e.to_string())?;
    let unpacked = out.join("_unzipped").join(id);
    let _ = std::fs::remove_dir_all(&unpacked);
    let status = std::process::Command::new("python3")
        .args(["-I", "-m", "zipfile", "-e"])
        .arg(&path)
        .arg(&unpacked)
        .status()
        .map_err(|e| format!("python3: {e}"))?;
    let result = if !status.success() {
        Err(format!("python3 -m zipfile -e exited {status}"))
    } else {
        let (got, want) = (files_under(&unpacked)?, files_under(case_dir)?);
        let mut diffs: Vec<String> = Vec::new();
        for (name, bytes) in &want {
            match got.get(name) {
                None => diffs.push(format!("{name} missing from the zip")),
                Some(b) if b != bytes => diffs.push(format!("{name} differs ({} vs {} bytes)", b.len(), bytes.len())),
                Some(_) => {}
            }
        }
        diffs.extend(got.keys().filter(|n| !want.contains_key(*n)).map(|n| format!("{n} only in the zip")));
        if diffs.is_empty() { Ok(()) } else { Err(diffs.join("; ")) }
    };
    let _ = std::fs::remove_dir_all(&unpacked);
    result
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

/// One `--dae-keep` case before it is written: `from` is `scene`, `single` or
/// a container, as in a `Case`.
struct KeepSpec {
    from: &'static str,
    dir: PathBuf,
    opts: WriteOptions,
    caps: Capabilities,
    prepared: Result<Scene, mesh::MeshError>,
    /// The bytes of `src/<from>` for a conversion; `None` for a direct write.
    source: Option<Vec<u8>>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(out) = args.first().map(PathBuf::from) else {
        eprintln!("usage: mesh_matrix <out_dir> [--fixtures a,b] [--formats glb,obj] [--import <dir>] [--transcode-textures] [--dae-keep] [--usd-variants]");
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
    let package_mode = WriteOptions { self_contained: false, ..Default::default() };
    // `--dae-keep`: the Collada directions once more, with the morphs kept.
    let dae_keep = args.iter().any(|a| a == "--dae-keep") && formats.contains(&MeshFormat::Dae);
    if args.iter().any(|a| a == "--dae-keep") && !dae_keep {
        eprintln!("--dae-keep adds nothing: `dae` is not in --formats, so no Collada is written");
    }
    // `--usd-variants`: the USDZ directions twice more, every clip, one with a USDC root.
    let usd_variants = args.iter().any(|a| a == "--usd-variants") && formats.contains(&MeshFormat::Usdz);
    if args.iter().any(|a| a == "--usd-variants") && !usd_variants {
        eprintln!("--usd-variants adds nothing: `usdz` is not in --formats, so no USDZ is written");
    }

    // The zip check's own control: entry names that are not ASCII (the UTF-8
    // flag) unpack under exactly those names.
    let utf8 = Package {
        files: ["model.obj", "textures/テクスチャ.png", "textures/ünïcödé.png"]
            .iter()
            .enumerate()
            .map(|(i, p)| PackageFile { path: p.to_string(), bytes: vec![i as u8; 3 + i] })
            .collect(),
    };
    let utf8_dir = out.join("_utf8_names");
    let _ = std::fs::remove_dir_all(&utf8_dir);
    write_package(&utf8_dir, &utf8).unwrap();
    if let Err(e) = zip_check(&out, "_utf8_names", &utf8, &utf8_dir) {
        eprintln!("the zip check's control failed: {e}");
        std::process::exit(1);
    }

    let mut sources: Vec<(String, Scene)> = fixtures::all()
        .into_iter()
        .filter(|(n, _)| only_fixtures.as_ref().is_none_or(|o| o.iter().any(|x| x == n)))
        .map(|(n, s)| (n.to_string(), s))
        .collect();

    // Third-party inputs: each becomes a fixture read by OUR reader. Its own
    // native rendering is the harness's independent reference for it.
    let mut imported: Vec<(String, String, PathBuf)> = Vec::new();
    // Third-party files our reader refused: each is a failure of the run, not
    // a row that quietly drops out of the matrix.
    let mut import_errors: Vec<Value> = Vec::new();
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
                    let mut scene = r.scene;
                    if args.iter().any(|a| a == "--transcode-textures") {
                        let report = mesh::normalize::transcode_textures(&mut scene);
                        for note in report.notes() {
                            eprintln!("{name}: {note}");
                        }
                    }
                    imported.push((name.clone(), ext.clone(), p.clone()));
                    sources.push((name, scene));
                }
                Err(e) => {
                    eprintln!("import {}: {e}", p.display());
                    import_errors.push(json!({ "file": p.display().to_string(), "format": fmt.as_str(), "error": e.to_string() }));
                }
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
                    if let Err(e) = zip_check(&out, &format!("{name}__scene__{}", to.as_str()), &w.package, &dir) {
                        case.error = Some(format!("zip check: {e}"));
                    }
                }
                Err(e) => case.error = Some(e.to_string()),
            }
            cases.push(case);
        }

        // Single-file writes, as the convert endpoint and a self-contained
        // delivery ship them: the containers whose one file differs from
        // their package (OBJ bakes colours into its vertices, glTF embeds
        // data URIs, Collada and USDA leave textures out). Each is rendered
        // against its own expected scene like any other case.
        let single = WriteOptions { self_contained: true, ..Default::default() };
        for &to in formats.iter().filter(|f| matches!(f, MeshFormat::Obj | MeshFormat::Gltf | MeshFormat::Dae | MeshFormat::Usda | MeshFormat::Usdc)) {
            let dir = fdir.join("src_single").join(to.as_str());
            let expected = want_expected(to.prepare(scene, single));
            let mut case = Case {
                expected_key: expected.clone().ok(),
                expected_error: expected.err(),
                fixture: name.clone(),
                from: "single".into(),
                to: to.as_str().into(),
                dir: dir.clone(),
                files: Vec::new(),
                caps: to.capabilities(single),
                losses: Vec::new(),
                warnings: Vec::new(),
                error: None,
            };
            match mesh::write_scene(scene, to, single) {
                Ok(w) => {
                    assert_eq!(w.package.files.len(), 1, "{name}: a single-file {to:?} wrote {} files", w.package.files.len());
                    case.files = write_package(&dir, &w.package).unwrap();
                    case.losses = w.losses.iter().map(|l| l.to_string()).collect();
                    if let Err(e) = zip_check(&out, &format!("{name}__single__{}", to.as_str()), &w.package, &dir) {
                        case.error = Some(format!("zip check: {e}"));
                    }
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
                                let id = format!("{name}__{}__{}", from.as_str(), to.as_str());
                                if let Err(e) = zip_check(&out, &id, &w.package, &dir) {
                                    case.error = Some(format!("zip check: {e}"));
                                }
                            }
                            Err(e) => case.error = Some(format!("write {}: {e}", to.as_str())),
                        }
                    }
                }
                cases.push(case);
            }
        }

        // `--dae-keep` (Ruling DM15): every direction into Collada once more,
        // written with the morphs kept. The sources are the ones above (a
        // `dae-keep` file is never read back), so only the writing and the
        // expected scene differ: `dae::prepare` under `Keep`.
        if dae_keep {
            let to = MeshFormat::Dae;
            let keep = WriteOptions { dae_morphs: DaeMorphs::Keep, ..package_mode };
            let keep_single = WriteOptions { dae_morphs: DaeMorphs::Keep, ..single };
            let mut keep_cases = vec![
                KeepSpec {
                    from: "scene",
                    dir: fdir.join("src").join("dae-keep"),
                    opts: keep,
                    caps: to.capabilities(keep),
                    prepared: to.prepare(scene, keep),
                    source: None,
                },
                KeepSpec {
                    from: "single",
                    dir: fdir.join("src_single").join("dae-keep"),
                    opts: keep_single,
                    caps: to.capabilities(keep_single),
                    prepared: to.prepare(scene, keep_single),
                    source: None,
                },
            ];
            for &from in &formats {
                let sdir = fdir.join("src").join(from.as_str());
                let primary = cases
                    .iter()
                    .find(|c| c.fixture == *name && c.from == "scene" && c.to == from.as_str())
                    .and_then(|c| c.files.first().cloned());
                keep_cases.push(KeepSpec {
                    from: from.as_str(),
                    dir: fdir.join("conv").join(format!("{}__dae-keep", from.as_str())),
                    opts: keep,
                    caps: from.capabilities(package_mode).intersect(to.capabilities(keep)),
                    prepared: from.prepare(scene, package_mode).and_then(|s| to.prepare(&s, keep)),
                    // The source's bytes, when it was written; read below.
                    source: primary.map(|p| sdir.join(p)).and_then(|p| std::fs::read(p).ok()),
                });
            }
            for KeepSpec { from, dir, opts, caps, prepared, source } in keep_cases {
                let expected = want_expected(prepared);
                let mut case = Case {
                    expected_key: expected.clone().ok(),
                    expected_error: expected.err(),
                    fixture: name.clone(),
                    from: from.into(),
                    to: "dae-keep".into(),
                    dir: dir.clone(),
                    files: Vec::new(),
                    caps,
                    losses: Vec::new(),
                    warnings: Vec::new(),
                    error: None,
                };
                let id = format!("{name}__{from}__dae-keep");
                let written = match from {
                    "scene" | "single" => mesh::write_scene(scene, to, opts).map_err(|e| e.to_string()),
                    _ => match (MeshFormat::parse(from), &source) {
                        (Some(fmt), Some(bytes)) => {
                            let side = DirSidecars(fdir.join("src").join(from));
                            match mesh::read_scene(bytes, fmt, &side) {
                                Err(e) => Err(format!("read {from}: {e}")),
                                Ok(r) => {
                                    case.warnings = r.warnings;
                                    // Named as the plain case names its refusal: by the format.
                                    mesh::write_scene(&r.scene, to, opts).map_err(|e| format!("write dae: {e}"))
                                }
                            }
                        }
                        _ => Err(format!("no {from} source was written")),
                    },
                };
                match written {
                    Ok(w) => {
                        if from == "single" {
                            assert_eq!(w.package.files.len(), 1, "{name}: a single-file dae-keep wrote {} files", w.package.files.len());
                        }
                        case.files = write_package(&dir, &w.package).unwrap();
                        case.losses = w.losses.iter().map(|l| l.to_string()).collect();
                        if let Err(e) = zip_check(&out, &id, &w.package, &dir) {
                            case.error = Some(format!("zip check: {e}"));
                        }
                    }
                    Err(e) => case.error = Some(e),
                }
                cases.push(case);
            }
        }

        // `--usd-variants` (mesh-gaps item 6): every direction into USDZ three
        // times more, as `dae-keep` does for Collada. `usdz-all` and `usdz-crate`
        // keep every clip (`usd_clips: all`, which a USDZ does not by default),
        // `usdz-crate` and `usdz-crate-first` root the package at a USDC
        // (`usdz_root: usdc`), the second with the default single clip. The
        // sources are the ones above, so only the writing and the expected scene
        // differ.
        if usd_variants {
            let to = MeshFormat::Usdz;
            let variants = [
                ("usdz-all", WriteOptions { usd_clips: Some(UsdClips::All), ..package_mode }),
                ("usdz-crate", WriteOptions { usd_clips: Some(UsdClips::All), usdz_root: UsdzRoot::Usdc, ..package_mode }),
                // The plain crate-rooted USDZ: a USDC root with the default
                // single clip, which a device check needs.
                ("usdz-crate-first", WriteOptions { usdz_root: UsdzRoot::Usdc, ..package_mode }),
            ];
            for (vname, opts) in variants {
                let mut specs = vec![KeepSpec {
                    from: "scene",
                    dir: fdir.join("src").join(vname),
                    opts,
                    caps: to.capabilities(opts),
                    prepared: to.prepare(scene, opts),
                    source: None,
                }];
                for &from in &formats {
                    let sdir = fdir.join("src").join(from.as_str());
                    let primary = cases
                        .iter()
                        .find(|c| c.fixture == *name && c.from == "scene" && c.to == from.as_str())
                        .and_then(|c| c.files.first().cloned());
                    specs.push(KeepSpec {
                        from: from.as_str(),
                        dir: fdir.join("conv").join(format!("{}__{vname}", from.as_str())),
                        opts,
                        caps: from.capabilities(package_mode).intersect(to.capabilities(opts)),
                        prepared: from.prepare(scene, package_mode).and_then(|s| to.prepare(&s, opts)),
                        source: primary.map(|p| sdir.join(p)).and_then(|p| std::fs::read(p).ok()),
                    });
                }
                for KeepSpec { from, dir, opts, caps, prepared, source } in specs {
                    let expected = want_expected(prepared);
                    let mut case = Case {
                        expected_key: expected.clone().ok(),
                        expected_error: expected.err(),
                        fixture: name.clone(),
                        from: from.into(),
                        to: vname.into(),
                        dir: dir.clone(),
                        files: Vec::new(),
                        caps,
                        losses: Vec::new(),
                        warnings: Vec::new(),
                        error: None,
                    };
                    let id = format!("{name}__{from}__{vname}");
                    let written = if from == "scene" {
                        mesh::write_scene(scene, to, opts).map_err(|e| e.to_string())
                    } else {
                        match (MeshFormat::parse(from), &source) {
                            (Some(fmt), Some(bytes)) => {
                                let side = DirSidecars(fdir.join("src").join(from));
                                match mesh::read_scene(bytes, fmt, &side) {
                                    Err(e) => Err(format!("read {from}: {e}")),
                                    Ok(r) => {
                                        case.warnings = r.warnings;
                                        mesh::write_scene(&r.scene, to, opts).map_err(|e| format!("write usdz: {e}"))
                                    }
                                }
                            }
                            _ => Err(format!("no {from} source was written")),
                        }
                    };
                    match written {
                        Ok(w) => {
                            case.files = write_package(&dir, &w.package).unwrap();
                            case.losses = w.losses.iter().map(|l| l.to_string()).collect();
                            if let Err(e) = zip_check(&out, &id, &w.package, &dir) {
                                case.error = Some(format!("zip check: {e}"));
                            }
                        }
                        Err(e) => case.error = Some(e),
                    }
                    cases.push(case);
                }
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
    let refused_imports = import_errors.len();
    let manifest = json!({
        "formats": formats.iter().map(|f| f.as_str()).collect::<Vec<_>>(),
        "fixtures": Value::Object(manifest_fixtures),
        "cases": cases_json,
        "import_errors": import_errors,
    });
    std::fs::write(out.join("manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    println!(
        "{} cases, {} conversion errors, {} refused as the expected scene is, {} imports refused -> {}",
        cases.len(),
        errors,
        refused,
        refused_imports,
        out.join("manifest.json").display()
    );
    for c in cases.iter().filter(|c| c.error.is_some()) {
        let why = c.expected_error.as_deref().map(|e| format!(" (expected: {e})")).unwrap_or_default();
        println!("  {} {} -> {}: {}{why}", c.fixture, c.from, c.to, c.error.as_deref().unwrap_or(""));
    }
}
