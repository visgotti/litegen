//! Minimal glTF 2.0 binary (`.glb`) writer.
//!
//! Produces a real, loadable unit cube rather than a placeholder, so the mock
//! 3D provider hands back bytes `three.js`'s `GLTFLoader` actually accepts.
//! The prompt seeds the base colour and a small scale jitter, so two prompts
//! never yield identical bytes — that is what proves the prompt reached the
//! provider.
//!
//! @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#binary-gltf-layout>

/// A cube is 12 triangles (2 per face).
pub const CUBE_TRIANGLE_COUNT: u32 = 12;

const POSITION_COUNT: usize = 8;
const INDEX_COUNT: usize = (CUBE_TRIANGLE_COUNT * 3) as usize;

/// FNV-1a — a stable, dependency-free hash so output is reproducible across
/// runs and platforms (`DefaultHasher` is explicitly not stable).
fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Build a valid GLB containing a single indexed, materialised cube.
pub fn generate_cube_glb(prompt: &str) -> Vec<u8> {
    let h = fnv1a(prompt);

    // Prompt-seeded colour (kept away from 0.0/1.0 so it is visibly shaded) and
    // a ±10% scale jitter. Both live in the JSON/BIN payload, so distinct
    // prompts differ byte-wise at identical length.
    let r = 0.15 + ((h & 0xff) as f32 / 255.0) * 0.8;
    let g = 0.15 + (((h >> 8) & 0xff) as f32 / 255.0) * 0.8;
    let b = 0.15 + (((h >> 16) & 0xff) as f32 / 255.0) * 0.8;
    let scale = 0.45 + (((h >> 24) & 0xff) as f32 / 255.0) * 0.10;

    // ─── BIN chunk: 8 positions (f32x3) then 36 indices (u16) ───────────────
    let corners: [[f32; 3]; POSITION_COUNT] = [
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
        [-1.0, -1.0,  1.0], [1.0, -1.0,  1.0], [1.0, 1.0,  1.0], [-1.0, 1.0,  1.0],
    ];
    let mut bin: Vec<u8> = Vec::with_capacity(POSITION_COUNT * 12 + INDEX_COUNT * 2);
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for c in corners {
        for axis in 0..3 {
            let v = c[axis] * scale;
            min[axis] = min[axis].min(v);
            max[axis] = max[axis].max(v);
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    let positions_len = bin.len(); // 96

    #[rustfmt::skip]
    const INDICES: [u16; INDEX_COUNT] = [
        0, 1, 2,  0, 2, 3, // -Z
        4, 6, 5,  4, 7, 6, // +Z
        0, 4, 5,  0, 5, 1, // -Y
        3, 2, 6,  3, 6, 7, // +Y
        0, 3, 7,  0, 7, 4, // -X
        1, 5, 6,  1, 6, 2, // +X
    ];
    for i in INDICES {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    let indices_len = bin.len() - positions_len; // 72
    pad_to_4(&mut bin, 0x00);

    // ─── JSON chunk ─────────────────────────────────────────────────────────
    let doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "litegen mock 3d" },
        "scene": 0,
        "scenes": [ { "nodes": [0] } ],
        "nodes": [ { "mesh": 0, "name": "cube" } ],
        "meshes": [ {
            "name": "cube",
            "primitives": [ { "attributes": { "POSITION": 0 }, "indices": 1, "material": 0 } ]
        } ],
        "materials": [ {
            "name": "generated",
            "pbrMetallicRoughness": {
                "baseColorFactor": [r, g, b, 1.0],
                "metallicFactor": 0.1,
                "roughnessFactor": 0.8
            }
        } ],
        "accessors": [
            {
                "bufferView": 0, "componentType": 5126, "count": POSITION_COUNT,
                "type": "VEC3", "min": min, "max": max
            },
            {
                "bufferView": 1, "componentType": 5123, "count": INDEX_COUNT,
                "type": "SCALAR"
            }
        ],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": positions_len, "target": 34962 },
            { "buffer": 0, "byteOffset": positions_len, "byteLength": indices_len, "target": 34963 }
        ],
        "buffers": [ { "byteLength": bin.len() } ]
    });
    let mut json = serde_json::to_vec(&doc).expect("glb json is always serializable");
    pad_to_4(&mut json, b' '); // JSON chunks pad with spaces, BIN with zeros.

    // ─── Container ──────────────────────────────────────────────────────────
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn pad_to_4(buf: &mut Vec<u8>, filler: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(filler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
    }

    #[test]
    fn emits_a_well_formed_glb_container() {
        let glb = generate_cube_glb("a low-poly fox");

        // 12-byte header: magic "glTF", version 2, total length == buffer len.
        assert_eq!(&glb[0..4], b"glTF", "GLB magic");
        assert_eq!(u32_at(&glb, 4), 2, "glTF version");
        assert_eq!(u32_at(&glb, 8) as usize, glb.len(), "declared length matches actual");

        // Chunk 0 must be JSON, 4-byte aligned.
        let json_len = u32_at(&glb, 12) as usize;
        assert_eq!(&glb[16..20], b"JSON", "first chunk type");
        assert_eq!(json_len % 4, 0, "JSON chunk must be 4-byte aligned");

        // Chunk 1 must be BIN\0, 4-byte aligned.
        let bin_off = 20 + json_len;
        let bin_len = u32_at(&glb, bin_off) as usize;
        assert_eq!(&glb[bin_off + 4..bin_off + 8], b"BIN\0", "second chunk type");
        assert_eq!(bin_len % 4, 0, "BIN chunk must be 4-byte aligned");
        assert_eq!(bin_off + 8 + bin_len, glb.len(), "no trailing bytes");
    }

    #[test]
    fn json_chunk_describes_one_indexed_mesh() {
        let glb = generate_cube_glb("cube");
        let json_len = u32_at(&glb, 12) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&glb[20..20 + json_len]).expect("JSON chunk parses");

        assert_eq!(json["asset"]["version"], "2.0");
        assert_eq!(json["scenes"].as_array().unwrap().len(), 1);
        assert_eq!(json["meshes"].as_array().unwrap().len(), 1);
        // GLB stores its buffer in the BIN chunk, so buffers[0] has no uri.
        assert!(json["buffers"][0].get("uri").is_none(), "GLB buffer must be chunk-backed");
        let prim = &json["meshes"][0]["primitives"][0];
        assert!(prim["attributes"]["POSITION"].is_number());
        assert!(prim["indices"].is_number(), "mesh must be indexed");
        assert!(prim["material"].is_number());

        // glTF REQUIRES min/max on a POSITION accessor — a loader rejects the file
        // without them, so this is a spec obligation, not a nicety.
        let pos_accessor = prim["attributes"]["POSITION"].as_u64().unwrap() as usize;
        let pos = &json["accessors"][pos_accessor];
        assert_eq!(pos["min"].as_array().unwrap().len(), 3, "POSITION min must be VEC3");
        assert_eq!(pos["max"].as_array().unwrap().len(), 3, "POSITION max must be VEC3");
        for axis in 0..3 {
            assert!(
                pos["min"][axis].as_f64().unwrap() < pos["max"][axis].as_f64().unwrap(),
                "POSITION bounds must be ordered on axis {axis}"
            );
        }
    }

    #[test]
    fn distinct_prompts_produce_distinct_bytes() {
        // Parity with the video mock's "keyframe blend must differ from the
        // prompt-only fallback" assertion: the mock must prove the prompt actually
        // reached the generator.
        let a = generate_cube_glb("prompt one");
        let b = generate_cube_glb("prompt two");
        assert_ne!(a, b, "prompt must vary the output");
        // Deliberately NOT asserting equal lengths. serde_json emits the shortest
        // round-trippable decimal, so the prompt-seeded floats (and the min/max
        // arrays derived from `scale`) differ in digit count between prompts —
        // measured 847-856 bytes unpadded across 30 prompts. Nothing consumes a
        // fixed mesh size; distinctness, determinism and container validity are
        // the invariants that matter, and each has its own test.
    }

    #[test]
    fn generation_is_deterministic() {
        assert_eq!(generate_cube_glb("same"), generate_cube_glb("same"));
    }

    #[test]
    fn triangle_count_matches_the_declared_constant() {
        let glb = generate_cube_glb("cube");
        let json_len = u32_at(&glb, 12) as usize;
        let json: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        let idx_accessor = json["meshes"][0]["primitives"][0]["indices"].as_u64().unwrap() as usize;
        let count = json["accessors"][idx_accessor]["count"].as_u64().unwrap() as u32;
        assert_eq!(count, CUBE_TRIANGLE_COUNT * 3);
    }
}
