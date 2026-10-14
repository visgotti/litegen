//! Mechanical enforcement that `mesh` stays decoupled from the rest of litegen.
//!
//! The whole value of this module is that it is bytes in, bytes out: directly
//! unit-testable without booting a router, a database or a provider, and
//! liftable into its own crate the day that is worth doing. That property is
//! easy to state in a doc comment and easy to lose — one `use crate::types::…`
//! in a hurry and the module is welded to the rest of the codebase, with no
//! test failing to say so.
//!
//! So it is a test. Adding a litegen dependency here fails CI, and the failure
//! names the line.

use std::path::Path;

/// Files that make up the module. Listed explicitly rather than globbed so that
/// a new codec has to be added here too — which is the moment to ask whether it
//  belongs.
const MODULE_FILES: &[&str] = &[
    "mod.rs",
    "ir.rs",
    "math.rs",
    "scene.rs",
    "pose.rs",
    "package.rs",
    "fixtures.rs",
    "zip.rs",
    "xml.rs",
    "glb.rs",
    "obj.rs",
    "stl.rs",
    "threemf.rs",
    "dae.rs",
    "fbx.rs",
    "fbx_dom.rs",
    "fbx_read.rs",
    "fbx_write.rs",
    "ply.rs",
    "usd.rs",
    "usd_read.rs",
    "usd_stage.rs",
    "usd_write.rs",
    "usda.rs",
    "usdc.rs",
    "tests.rs",
    "matrix_tests.rs",
    "scene_matrix_tests.rs",
    "isolation_tests.rs",
];

/// Excluded from both scans below, for the obvious reason.
const SELF: &str = "isolation_tests.rs";

fn module_dir() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/mesh"))
}

#[test]
fn the_mesh_module_imports_nothing_from_litegen() {
    let mut violations: Vec<String> = Vec::new();

    for name in MODULE_FILES {
        // This file is the scanner, not the scanned: it necessarily contains
        // the very strings it looks for.
        if *name == SELF {
            continue;
        }
        let path = module_dir().join(name);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is listed in MODULE_FILES but unreadable: {e}", path.display()));

        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("").trim();
            if code.contains("crate::") || code.contains("::crate") {
                violations.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "mesh must not depend on the rest of litegen — it is bytes in, bytes out, and stays \
         directly unit-testable and crate-extractable only while that holds.\n\
         Convert at the boundary (proxy/router) instead, passing plain bytes in.\n\n{}",
        violations.join("\n"),
    );
}

#[test]
fn every_module_file_is_declared() {
    // The mirror of the list above: a file on disk that nobody declared is
    // either dead code or an undeclared dependency surface, and both should be
    // noticed rather than discovered later.
    let mut on_disk: Vec<String> = std::fs::read_dir(module_dir())
        .expect("mesh module dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs"))
        .collect();
    on_disk.sort();

    let mut declared: Vec<String> = MODULE_FILES.iter().map(|s| s.to_string()).collect();
    declared.sort();

    assert_eq!(
        on_disk, declared,
        "files in src/mesh do not match MODULE_FILES — add the new file to the list (and to \
         mod.rs), or delete it",
    );
}

#[test]
fn third_party_crates_stay_in_the_one_file_that_needs_each() {
    // The code that parses untrusted vendor bytes is std-only by default: a new
    // crate there is a new supply-chain surface on the poll path. The
    // exceptions are crates that are ALREADY compiled into litegen through
    // other dependencies (so they add no code to the binary), each confined to
    // the one file whose format genuinely needs it:
    //
    // - serde_json: glTF's JSON chunk is genuinely JSON (glb.rs).
    // - miniz_oxide: DEFLATE, for ZIP entries (zip.rs) and FBX's zlib-compressed
    //   arrays (fbx.rs). Already linked through flate2 (png, tower-http).
    // - quick_xml: 3MF and Collada are XML (xml.rs, which the codecs use).
    //   Already linked through rust-s3.
    //
    // `matrix_tests.rs` may use serde_json to construct glTF documents field by
    // field — fixture construction, not a dependency the module carries.
    const ALLOWED: &[(&str, &[&str])] = &[
        ("serde_json", &["glb.rs", "matrix_tests.rs"]),
        ("miniz_oxide", &["zip.rs", "fbx.rs"]),
        ("quick_xml", &["xml.rs"]),
    ];
    for name in MODULE_FILES {
        if *name == SELF {
            continue;
        }
        let src = std::fs::read_to_string(module_dir().join(name)).unwrap();
        for (n, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("").trim();
            for (krate, files) in ALLOWED {
                assert!(
                    !code.contains(krate) || files.contains(name),
                    "{name}:{}: only {files:?} may use {krate}", n + 1,
                );
            }
        }
    }
}
