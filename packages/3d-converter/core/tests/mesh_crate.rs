//! `litegen-mesh` is litegen-core's `src/mesh/` compiled on its own. These
//! tests keep it the library litegen runs: the same dependency specs and the
//! same resolved versions, so the packages write the bytes litegen writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The crates `mesh/` may use (`.claude/rules/mesh-standalone.md`).
/// The crates mesh/ may use, as mesh's own isolation test lists them
/// (`ALLOWED` in litegen-core/src/mesh/isolation_tests.rs), spelled as
/// Cargo.toml keys: that test, not a copy here, is the authority.
fn crates_mesh_uses() -> Vec<String> {
    let source = std::fs::read_to_string(repo().join("litegen-core/src/mesh/isolation_tests.rs")).unwrap();
    let block = &source[source.find("const ALLOWED").expect("isolation_tests.rs has ALLOWED")..];
    let block = &block[..block.find("];").unwrap()];
    let used: Vec<String> = block.lines().filter_map(|l| l.trim().strip_prefix("(\"")?.split('"').next()).map(|c| c.trim_end_matches("::").to_string()).collect();
    let core = read_toml("litegen-core/Cargo.toml");
    let mut deps: Vec<String> =
        core["dependencies"].as_table().unwrap().keys().filter(|k| used.contains(&k.replace('-', "_"))).cloned().collect();
    deps.sort();
    assert_eq!(deps.len(), used.len(), "every crate in ALLOWED is a litegen-core dependency: {used:?} vs {deps:?}");
    deps
}

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn read_toml(rel: &str) -> toml::Table {
    let path = repo().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    toml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn mesh_deps_match_litegen_core() {
    let core = read_toml("litegen-core/Cargo.toml");
    let mesh = read_toml("packages/3d-converter/mesh/Cargo.toml");
    let core_deps = core["dependencies"].as_table().unwrap();
    let mesh_deps = mesh["dependencies"].as_table().unwrap();
    let names: Vec<String> = mesh_deps.keys().cloned().collect();
    let want = crates_mesh_uses();
    assert_eq!(names, want, "litegen-mesh depends on exactly the crates mesh/ uses");
    for name in want.iter().map(String::as_str) {
        assert_eq!(
            mesh_deps[name], core_deps[name],
            "`{name}` in packages/3d-converter/mesh/Cargo.toml differs from litegen-core/Cargo.toml"
        );
    }
}

/// `(name, version)` of every package `root` depends on, transitively, in a
/// parsed `Cargo.lock`.
fn closure(lock: &toml::Table, root: &str) -> BTreeSet<(String, String)> {
    let packages = lock["package"].as_array().unwrap();
    let mut by_name: BTreeMap<&str, Vec<&toml::Table>> = BTreeMap::new();
    for p in packages {
        let p = p.as_table().unwrap();
        by_name.entry(p["name"].as_str().unwrap()).or_default().push(p);
    }
    let find = |entry: &str| -> &toml::Table {
        // A dependency entry is "name", or "name version" when the lock holds
        // several versions of it, optionally followed by a source.
        let mut parts = entry.split(' ');
        let name = parts.next().unwrap();
        let candidates = &by_name[name];
        match parts.next() {
            Some(version) => candidates.iter().find(|p| p["version"].as_str() == Some(version)).unwrap(),
            None => {
                assert_eq!(candidates.len(), 1, "{name} is locked at several versions");
                candidates[0]
            }
        }
    };
    let mut seen = BTreeSet::new();
    let mut stack = vec![find(root)];
    while let Some(p) = stack.pop() {
        let key = (p["name"].as_str().unwrap().to_string(), p["version"].as_str().unwrap().to_string());
        if !seen.insert(key) {
            continue;
        }
        if let Some(deps) = p.get("dependencies").and_then(|d| d.as_array()) {
            stack.extend(deps.iter().map(|d| find(d.as_str().unwrap())));
        }
    }
    seen.remove(&(root.to_string(), env!("CARGO_PKG_VERSION").to_string()));
    seen
}

#[test]
fn mesh_lock_versions_match_litegen_core() {
    let ours = read_toml("packages/3d-converter/Cargo.lock");
    let theirs = read_toml("litegen-core/Cargo.lock");
    let theirs: BTreeSet<(String, String)> = theirs["package"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["name"].as_str().unwrap().to_string(), p["version"].as_str().unwrap().to_string()))
        .collect();
    let mesh = closure(&ours, "litegen-mesh");
    assert!(mesh.iter().any(|(n, _)| n == "image"), "the closure reaches mesh's dependencies");
    let drifted: Vec<String> = mesh.iter().filter(|p| !theirs.contains(*p)).map(|(n, v)| format!("{n} {v}")).collect();
    assert!(
        drifted.is_empty(),
        "litegen-mesh resolves versions litegen-core doesn't lock: {drifted:?}; \
         align each with `cargo update -p <name> --precise <litegen-core's version>`"
    );
}

#[test]
fn the_mesh_crate_round_trips_a_model() {
    // The crate compiled from mesh/ reads and writes: a glTF of the cube
    // fixture (one mesh, one node), converted to OBJ and read back, keeps
    // every triangle.
    use litegen_mesh::{fixtures, inspect, read_scene, write_scene, MeshFormat, NoSidecars, WriteOptions};
    let scene = fixtures::cube();
    let glb = write_scene(&scene, MeshFormat::Glb, WriteOptions::default()).unwrap().package.files.remove(0).bytes;
    let obj = litegen_mesh::convert(&glb, MeshFormat::Glb, MeshFormat::Obj).unwrap();
    let back = read_scene(&obj, MeshFormat::Obj, &NoSidecars).unwrap().scene;
    assert_eq!(inspect::inspect(&back).unwrap().triangle_count, scene.triangle_count());
}

/// The clippy lints litegen-core allows on `mod mesh` (litegen-core/src/lib.rs),
/// which litegen-mesh, as mesh's own crate, must allow too.
fn litegen_core_mesh_allows() -> Vec<String> {
    let lib = std::fs::read_to_string(repo().join("litegen-core/src/lib.rs")).unwrap();
    let at = lib.find("pub mod mesh;").expect("litegen-core declares mod mesh");
    let open = lib[..at].rfind("#[allow(").expect("an allow list on mod mesh");
    lib[open..at]
        .split(|c: char| c == ',' || c == '(' || c == ')' || c.is_whitespace())
        .filter_map(|t| t.strip_prefix("clippy::"))
        .map(str::to_string)
        .collect()
}

#[test]
fn mesh_lints_match_litegen_core() {
    let mesh = read_toml("packages/3d-converter/mesh/Cargo.toml");
    let ours: BTreeSet<String> = mesh
        .get("lints")
        .and_then(|l| l.get("clippy"))
        .and_then(|c| c.as_table())
        .map(|t| t.iter().filter(|(_, v)| v.as_str() == Some("allow")).map(|(k, _)| k.clone()).collect())
        .unwrap_or_default();
    let theirs: BTreeSet<String> = litegen_core_mesh_allows().into_iter().collect();
    assert!(theirs.contains("neg_cmp_op_on_partial_ord"), "parsed litegen-core's list: {theirs:?}");
    assert_eq!(ours, theirs, "mesh/Cargo.toml [lints.clippy] must allow exactly what litegen-core allows on mod mesh");
}
