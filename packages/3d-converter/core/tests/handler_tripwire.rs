//! A deliberate change detector (the spec's section 7.3). The core copies
//! items of litegen's HTTP converter (PARAMS, UNITLESS, convert_bytes,
//! parse_query) and its media-type table (store/mod.rs, model3d_content_type;
//! proxy/storage.rs until the engine boundary moved it on 2026-11-07),
//! which live outside `mesh/` where no exhaustive match can see them change.
//! When any of them changes, this fails, so the copy is re-checked rather
//! than left to drift.

use std::path::Path;

/// FNV-1a 64 of the watched items' source text, as last checked. Re-checked
/// 2026-11-08: for `ktx2_textures` (the core takes no `keep`, so it writes as
/// the default, `png`, does, and names what transcoding leaves out exactly as
/// `convert_bytes` does); then for `fbx_light_units` and `usd_light_units`
/// (`PARAMS` 11; one choice per format given to both the read and the write,
/// as the core's `convert` takes it; refused at a wrong end with the core's
/// texts; write.rs `light_units_equal_the_hosted_pipeline`). Re-checked
/// 2026-11-09 for `dae_morphs`, the only change (the items with its lines
/// taken out hash to the previous pin): `PARAMS` 12; refused unless `to` is
/// dae, with the core's text; `bake` or `keep` in any case and spacing, as the
/// core's `DaeMorphs::parse` reads them; given to the write as the core's
/// `convert` gives it (write.rs `dae_morphs_equal_the_hosted_pipeline`).
/// Re-checked 2026-11-10 for mesh-gaps item 6, the only change (the items
/// with its Task 2a diff reversed hash to the previous pin): `PARAMS` 14;
/// `usd_light_units` at a usdc end too, "usdz, usda or usdc"; `usdz_root`
/// refused unless `to` is usdz and `usd_clips` unless `to` is usda, usdc or
/// usdz, with the core's texts; `usda`/`usdc` and `all`/`first` in any case
/// and spacing, as the core's `parse` reads them; given to the write as the
/// core's `convert` gives them, `usdz_root` defaulting to usda and `usd_clips`
/// left to mesh (write.rs `usd_choices_equal_the_hosted_pipeline`); usdc
/// served as `application/octet-stream`.
const PINNED: u64 = 0x1a47_8eea_b11d_1e00;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The text of the item starting at `start`: its line through the first
/// closing brace at column 0 (a whole `fn`), or through the `;` that ends it
/// (a `const`, which rustfmt spreads over several lines once it is long, as
/// it did `PARAMS`: its first line alone would miss a renamed parameter).
fn item(source: &str, start: &str) -> String {
    let at = source.find(start).unwrap_or_else(|| panic!("convert.rs has no `{start}`"));
    let rest = &source[at..];
    if start.starts_with("const ") {
        let end = rest.find(";\n").unwrap_or_else(|| panic!("`{start}` has no `;` ending a line"));
        return rest[..=end].to_string();
    }
    let end = rest.find("\n}\n").unwrap_or_else(|| panic!("`{start}` has no closing brace at column 0"));
    rest[..end + 2].to_string()
}

#[test]
fn hosted_converter_logic_is_unchanged() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../litegen-core/src/api/handlers/convert.rs");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let storage_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../litegen-core/src/store/mod.rs");
    let storage = std::fs::read_to_string(&storage_path).unwrap_or_else(|e| panic!("{}: {e}", storage_path.display()));
    let mut watched = ["const PARAMS", "const UNITLESS", "fn convert_bytes(", "fn parse_query("].map(|s| item(&source, s)).to_vec();
    watched.push(item(&storage, "pub fn model3d_content_type<"));
    // The error mapping the core copies (error.rs `From<MeshError>`, model.rs
    // `mismatch_names_from`) and the format parsing parse_query calls.
    watched.extend(["fn mesh_error_response(", "fn parse_format("].map(|s| item(&source, s)));
    let watched = watched.join("\n---\n");
    let hash = fnv1a64(watched.as_bytes());
    assert_eq!(
        hash, PINNED,
        "litegen's converter handler changed (PARAMS / UNITLESS / convert_bytes / parse_query / \
         parse_format / mesh_error_response, or store/mod.rs's model3d_content_type): re-check \
         core/src/model.rs (write, media types, mismatch_names_from), core/src/error.rs (the error \
         codes) and core/src/model.rs + input.rs + options.rs (the option rules) against it, then set PINNED to {hash:#x}"
    );
}
