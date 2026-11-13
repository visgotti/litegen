//! glTF 2.0 — binary (`.glb`) and JSON (`.gltf`) — read and write.
//!
//! # Why this reader is the paranoid one
//!
//! GLB is the container every 3D vendor in the catalog actually emits, so these
//! bytes are the ones that arrive from outside. Everything here assumes the
//! input is a real exporter's output rather than our own writer's: a scene graph
//! with a baked root rotation, geometry split into one primitive per material,
//! interleaved vertex buffers, normalised integer attributes, sparse morph
//! targets, and chunk types we have never heard of. Reading any of that wrongly
//! does not fail — it hands the customer a model that is rotated, half-missing
//! or inside out — which is why the traversal honours the whole node graph, the
//! accessor layer decodes every encoding the spec allows, and anything it cannot
//! decode is refused by name rather than guessed at.
//!
//! The other half of the paranoia is the hostile case. A `count`, a
//! `byteLength` or a chunk length in this file is a 32-bit number chosen by
//! someone else, so every index and every length is checked before it is used,
//! and every decoded value is charged to a [`Budget`] so that a small file whose
//! accessors all alias one buffer view cannot decode into gigabytes. A truncated
//! or malicious file must come back as `Err`, never as a panic or an abort.
//!
//! # What is read
//!
//! The full document model this module's [`Scene`] mirrors: the node hierarchy
//! of the selected scene, every primitive (triangles, lines and points: modes
//! 0 to 6) with positions, normals, tangents, all UV sets, `COLOR_0`, every
//! `JOINTS_n`/`WEIGHTS_n` pair and morph targets; metallic-roughness materials
//! with `KHR_texture_transform`, `KHR_materials_emissive_strength` and
//! `KHR_materials_unlit`, and the PBR layers over them (`KHR_materials_clearcoat`, `_transmission`, `_volume`,
//! `_ior`, `_specular`, `_sheen`, `_iridescence`, `_anisotropy` and
//! `_dispersion`); textures, samplers and images (PNG, JPEG, and WebP/KTX2
//! carried as-is); skins; animations of translation, rotation, scale and
//! morph weights; and the cameras and `KHR_lights_punctual` lights its nodes
//! place, whose values the scene keeps in glTF's own units and axes, the
//! cameras in the file's order. A camera's view is scale-free in glTF: it is
//! "derived from the rotation and translation components of the node's
//! transformation matrix" (glTF 2.0, Cameras, View Matrix). So a camera
//! under a scaled ancestor (FBX's centimetre root, Collada's `<unit>`, USD's
//! `metersPerUnit`, `normalize::place`) is written where it stands, and
//! three.js r183 views it correctly: its `Camera` excludes the world scale
//! from the view matrix ("to be glTF conform", `src/cameras/Camera.js`
//! L116-L126; Ruling R33). An engine that applies node scale to a camera
//! would differ: it would see that camera's view scaled, near and far
//! planes with it.
//!
//! Dropped with a warning that names them: the extensions the scene does not
//! carry, such as `KHR_materials_variants`; a light or camera that does not
//! read or breaks a bound (`Light::check`, `Camera::check`), that a node
//! places past the file's array, or that no node of the scene places (a
//! reader never refuses a file over a light or camera, and the node stays:
//! mesh-gaps item 5), though three such lights are kept with a warning
//! instead: a hard-edged spot (equal cones, which glTF forbids) is softened
//! (`Light::soften_hard_edge`), a spot wider than a hemisphere is read as a
//! point light (Ruling R32), and a colour past 1 moves its peak into the
//! intensity (Ruling R8); a directional light's `range`, which glTF ignores
//! (and which `MeshFormat::prepare` drops from a scene glTF writes); and a
//! layer whose own JSON does not read, or that names a texture whose table
//! entry is malformed, which only a base map's use refuses (Ruling A16c,
//! revised).
//! The base layer under a dropped layer stays. A layer-only image that can't be
//! read for another reason (past the image budget, in an external buffer not
//! supplied, or in a `data:` URI that isn't base64) refuses the file, as a
//! base map's does (Rulings A47, A47a); one in a meshopt view that doesn't
//! decode drops its layer with a warning instead, as a malformed layer is
//! dropped (Ruling B15b). A refused value is quoted cut to 60 characters,
//! never serialized whole (Ruling A45).
//!
//! Points and lines (mesh-gaps item 10): POINTS (mode 0) reads as a points
//! primitive, and LINES, LINE_LOOP and LINE_STRIP (modes 1 to 3) as a lines
//! primitive of segments, each with the attributes, material, skin and morph
//! targets a triangle primitive has. A loop of n vertices expands to n
//! segments and a strip to n − 1, charged to the index budget at their
//! expanded size before anything is allocated; a loop or strip of fewer than
//! two vertices, and a LINES primitive with an odd number of indices, is
//! Malformed. Points with no `indices` get `0..n`. The writer writes modes 0
//! and 1, a list of points and a list of segments, never a loop or a strip. A
//! Draco-compressed primitive can't be in these modes (below).
//!
//! # Compressed geometry (mesh-gaps item 2)
//!
//! Both glTF compression extensions are decoded on read, so a compressed
//! file reads as its plain twin does. litegen's output is never compressed:
//! the writers write neither extension, and a delivered glTF requires none.
//!
//! - **meshopt**, `EXT_meshopt_compression` and `KHR_meshopt_compression`
//!   (`meshopt.rs`): every mode (ATTRIBUTES, TRIANGLES, INDICES), every
//!   filter (OCTAHEDRAL, QUATERNION, EXPONENTIAL, COLOR) and both attribute
//!   stream versions. A view is decoded the first time anything reads it
//!   and kept, so the accessors that share it decode it once. The v1
//!   attribute stream and the COLOR filter are KHR's; an EXT view that
//!   carries either is read as KHR's, with a warning. A view whose extension
//!   is only in `extensionsUsed` decodes too, never its fallback buffer
//!   (Ruling B22): a `.gltf` given its fallback `.bin` but not its stream's
//!   is refused as incomplete. A layer-only image in a meshopt view that
//!   doesn't decode drops its layer with a warning (Ruling B15b, above).
//! - **Draco**, `KHR_draco_mesh_compression` (`draco.rs`, through the
//!   `draco-core` crate, under the absolute `DRACO_LIMITS`): a TRIANGLES or
//!   TRIANGLE_STRIP primitive, whose stream's faces are read as a triangle
//!   list either way, since a Draco stream holds faces, not a strip
//!   (three.js expands a Draco strip's faces as a strip, GLTFLoader.js
//!   :3851-3853; no Khronos sample has one). Refused by name: a primitive
//!   in any other mode (POINTS, LINES, LINE_LOOP, LINE_STRIP or
//!   TRIANGLE_FAN), which the extension doesn't allow, as Malformed; a
//!   point cloud; a bitstream older than 2.2; and a stream past the
//!   limits. The stream is decoded even when the extension is only used,
//!   never the fallback accessors (Ruling B27). An accessor whose `count`
//!   disagrees with the decoded count, `indices` included, is read at the
//!   decoded count, with a warning (Ruling B32). An attribute the extension
//!   maps but the primitive doesn't list is skipped with a warning when this
//!   reader wouldn't read it, and refused when it would (Ruling B49).
//!
//! Ruling IDs ("Ruling A16a") are mesh-gaps item 4's rulings, listed with
//! their costs in the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-material-layers.md`. "Ruling R32"
//! and its kin are item 5's (lights and cameras), listed with their costs in
//! the "Rulings" section of
//! `docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md`. "Ruling B22"
//! and its kin are item 2's (Draco and meshopt), listed with their costs in
//! the status block of
//! `docs/superpowers/plans/2026-11-04-mesh-compressed-gltf-input.md`.
//!
//! @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html>

use super::ir::Mesh;
use super::math;
use super::package::{self, NoSidecars, Package, PackageFile, Sidecars};
use super::scene::{
    AlphaMode, AnimPath, Animation, Channel, Filter, Image, ImageFormat, Interpolation, Material,
    MorphTarget, Node, Primitive, Sampler, Scene, SceneMesh, Skin, SkinWeights, Texture, TextureRef,
    Transform, UvTransform, Wrap,
};
use super::scene::{Anisotropy, Clearcoat, Iridescence, MaterialLayer, Sheen, Specular, Transmission, Volume};
use super::scene::{Camera, Light, LightKind, Projection};
use super::draco;
use super::meshopt;
use super::{MeshError, MeshFormat};
use serde_json::Value;
use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;

// ─── container constants ────────────────────────────────────────────────────

const HEADER_LEN: usize = 12;
const MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A; // 'JSON'
const CHUNK_BIN: u32 = 0x004E_4942; // 'BIN\0'

// componentType enum values (they are GL type enums, not indices).
const COMP_BYTE: u32 = 5120;
const COMP_UNSIGNED_BYTE: u32 = 5121;
const COMP_SHORT: u32 = 5122;
const COMP_UNSIGNED_SHORT: u32 = 5123;
const COMP_UNSIGNED_INT: u32 = 5125;
const COMP_FLOAT: u32 = 5126;

/// `primitive.mode` for a triangle list. Absent means 4.
const MODE_TRIANGLES: u64 = 4;
/// Mode 5: the same triangles, sharing an edge with the triangle before.
const MODE_TRIANGLE_STRIP: u64 = 5;
/// Mode 6: the same triangles, all sharing vertex 0.
const MODE_TRIANGLE_FAN: u64 = 6;

const TARGET_ARRAY_BUFFER: u32 = 34962;
const TARGET_ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// Ceiling on the merged model, in vertices and in indices — the scene's own
/// ceilings, restated here because the flat API's callers and tests name them.
///
/// 4M is what a 128 MB GLB can honestly carry at the ceiling: interleaved
/// position, normal and uv is 32 bytes a vertex, so nothing legitimate that
/// passes `super::MAX_INPUT_BYTES` has more. Instancing is what multiplies a
/// small accessor past it, and `pose::world_primitives` enforces it there.
pub const MAX_MERGED_VERTICES: usize = super::scene::MAX_SCENE_VERTICES;
pub const MAX_MERGED_INDICES: usize = super::scene::MAX_SCENE_INDICES;

/// Ceiling on decoded attribute values (floats, or integer components) across
/// the whole document.
///
/// The bounds checks prove every accessor's bytes exist, but they cannot stop
/// a thousand accessors from aliasing the same view — each one is honest, and
/// together they decode to a thousand times the file. 48M values is ~192 MB of
/// `f32`: more than a 128 MB file of float attributes can hold, so nothing
/// legitimate reaches it. A compressed view is charged here beside the
/// accessors over it (its decoded bytes, then their `f32` copies), so a
/// compressed file's vertex ceiling is about half an uncompressed one's:
/// around 3M vertices at 32 bytes each, far above what any 3D vendor ships
/// (final review, Minor 3; a view budget of its own if one ever reaches it).
const MAX_DECODED_VALUES: usize = 48_000_000;

/// Ceiling on image bytes copied out of the document, for the same aliasing
/// reason: many `images` may name one `bufferView`.
const MAX_IMAGE_BYTES: usize = super::MAX_INPUT_BYTES;

/// UV sets read per primitive. glTF puts no bound on `TEXCOORD_n`; nothing
/// real uses more than two, and every other container keeps at most a few.
const MAX_UV_SETS: usize = 8;
/// `JOINTS_n`/`WEIGHTS_n` pairs read per primitive: 32 influences per vertex,
/// past anything a skinning shader evaluates.
const MAX_INFLUENCE_SETS: usize = 8;

/// Extensions that may appear in `extensionsRequired` without invalidating what
/// we extract — either because they only describe things outside the scene
/// model (material variants) or because this reader implements them.
/// Anything NOT on this list is refused: `extensionsRequired` means the file
/// cannot be interpreted correctly without it, and a compression extension
/// this reader didn't decode would otherwise leave us reading a zero-length
/// fallback buffer and reporting success on an empty model.
const IGNORABLE_REQUIRED_EXTENSIONS: &[&str] = &[
    "KHR_draco_mesh_compression",
    "KHR_lights_punctual",
    "KHR_materials_anisotropy",
    "KHR_materials_clearcoat",
    "KHR_materials_dispersion",
    "KHR_materials_emissive_strength",
    "KHR_materials_ior",
    "KHR_materials_iridescence",
    "KHR_materials_sheen",
    "KHR_materials_specular",
    "KHR_materials_transmission",
    "KHR_materials_unlit",
    "KHR_materials_variants",
    "KHR_materials_volume",
    "KHR_mesh_quantization",
    "KHR_meshopt_compression",
    "KHR_texture_basisu",
    "KHR_texture_transform",
    "EXT_meshopt_compression",
    "EXT_texture_webp",
];

/// Extensions this reader carries into the scene. Any other in
/// `extensionsUsed` is named in the read's warnings with what is lost, so a
/// conversion never drops material variants silently. The compression
/// extensions are here because they are decoded: nothing is lost.
const CARRIED_EXTENSIONS: &[&str] = &[
    "KHR_draco_mesh_compression",
    "KHR_lights_punctual",
    "KHR_materials_anisotropy",
    "KHR_materials_clearcoat",
    "KHR_materials_dispersion",
    "KHR_materials_emissive_strength",
    "KHR_materials_ior",
    "KHR_materials_iridescence",
    "KHR_materials_sheen",
    "KHR_materials_specular",
    "KHR_materials_transmission",
    "KHR_materials_unlit",
    "KHR_materials_volume",
    "KHR_mesh_quantization",
    "KHR_meshopt_compression",
    "KHR_texture_basisu",
    "KHR_texture_transform",
    "EXT_meshopt_compression",
    "EXT_texture_webp",
];

/// The meshopt extensions, both decoded by `meshopt.rs`. The writer writes
/// neither: litegen's glTF output stays uncompressed.
const MESHOPT_EXTENSIONS: [&str; 2] = ["EXT_meshopt_compression", "KHR_meshopt_compression"];

/// The Draco extension, whose primitives `draco.rs` decodes. The writer
/// doesn't write it either.
const DRACO_EXTENSION: &str = "KHR_draco_mesh_compression";

/// The compression extensions this reader decodes: [`MESHOPT_EXTENSIONS`]
/// and Draco's. This module's writers never compress, so a caller delivering
/// a file that uses one, even beside an uncompressed fallback, should
/// rewrite it rather than hand it on.
pub const COMPRESSION_EXTENSIONS: &[&str] = &[MESHOPT_EXTENSIONS[0], MESHOPT_EXTENSIONS[1], DRACO_EXTENSION];

/// The absolute limits on one Draco primitive's decode: the scene's own
/// vertex and index caps, and draco.rs's decoded-bytes ceiling, set to bound
/// the Draco decoder's memory on the shapes measured, strips and grids
/// (Ruling B38: 24 MB, under the document's own 192 MB budget, which still
/// holds what the scene copies; the faces-per-point pre-check that would
/// cover a mesh with many more faces a point is open, Ruling B39). A
/// stream's size bounds nothing (a constant attribute codes to a few bytes
/// whatever its count), so these hold whatever the file is.
const DRACO_LIMITS: draco::Limits =
    draco::Limits { max_points: MAX_MERGED_VERTICES, max_faces: MAX_MERGED_INDICES / 3, max_bytes: draco::MAX_DECODED_BYTES };

/// The meshopt extension objects `view` carries.
fn meshopt_extensions(view: &Value) -> impl Iterator<Item = (&'static str, &Value)> {
    let extensions = view.get("extensions");
    MESHOPT_EXTENSIONS.into_iter().filter_map(move |name| Some((name, extensions?.get(name)?)))
}

/// The glTF extension a material layer is read from and written as.
fn layer_extension(layer: MaterialLayer) -> &'static str {
    match layer {
        MaterialLayer::Clearcoat => "KHR_materials_clearcoat",
        MaterialLayer::Transmission => "KHR_materials_transmission",
        MaterialLayer::Volume => "KHR_materials_volume",
        MaterialLayer::Ior => "KHR_materials_ior",
        MaterialLayer::Specular => "KHR_materials_specular",
        MaterialLayer::Sheen => "KHR_materials_sheen",
        MaterialLayer::Iridescence => "KHR_materials_iridescence",
        MaterialLayer::Anisotropy => "KHR_materials_anisotropy",
        MaterialLayer::Dispersion => "KHR_materials_dispersion",
    }
}

/// What the scene loses with an extension it does not carry.
fn extension_loss(ext: &str) -> &'static str {
    match ext {
        "KHR_materials_variants" => "every material variant but the default",
        _ => "what that extension describes",
    }
}

/// Warnings for the extensions a document uses that the scene drops.
fn uncarried_extensions(doc: &Value) -> Vec<String> {
    let used: Vec<&str> = doc.get("extensionsUsed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    used.into_iter()
        .filter(|e| !CARRIED_EXTENSIONS.contains(e))
        .map(|e| format!("the file uses {e}, which litegen does not carry: {} is dropped", extension_loss(e)))
        .collect()
}

// ─── error helpers ──────────────────────────────────────────────────────────

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(detail.into())
}

fn not_glb(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Glb, detail: detail.into() }
}

// ─── little-endian scalar reads ─────────────────────────────────────────────

fn u32_le(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn as_usize(v: &Value) -> Option<usize> {
    usize::try_from(v.as_u64()?).ok()
}

/// An optional glTF index or byte count: absent means the caller's default,
/// present means it must read as one.
///
/// `.and_then(as_usize)` folds those two cases into one `None`, and
/// `as_u64` rejects every number serde_json parsed as a float — so a `24.0`
/// byteStride out of a Python exporter, or an `indices: 1.0`, arrived here
/// looking exactly like an omitted field and took the default branch. For
/// `indices` that default is a synthesised sequential list, which turns a file
/// saying [2,1,0] into [0,1,2]: reversed winding, model inside out, no error.
fn index_field(v: Option<&Value>, what: &str) -> Result<Option<usize>, MeshError> {
    match v {
        None => Ok(None),
        Some(v) => as_usize(v).map(Some).ok_or_else(|| malformed(format!("{what} is {}, which is not an index", shown_json(v, 60)))),
    }
}

/// `v` as JSON, cut to `max` characters and marked "…" when cut, for a
/// message. It is written into a buffer that stops serde_json once it holds
/// enough, so a value of megabytes is never serialized whole to be cut
/// (Ruling A45).
fn shown_json(v: &Value, max: usize) -> String {
    struct Upto(Vec<u8>, usize);
    impl std::io::Write for Upto {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let room = self.1 - self.0.len();
            if room == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            let n = b.len().min(room);
            self.0.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    // Four bytes a character at most, and a character more to tell a cut.
    let mut out = Upto(Vec::new(), 4 * (max + 1));
    let whole = serde_json::to_writer(&mut out, v).is_ok();
    let text = String::from_utf8_lossy(&out.0);
    let kept = truncate(&text, max);
    if whole && kept.len() == text.len() {
        kept.to_string()
    } else {
        format!("{kept}…")
    }
}

/// Whether a JSON number survives as an f32, the precision glTF's data and
/// the scene's attributes have: a finite number past f32's range (1e39)
/// would become infinity, and NaN in everything it touches.
fn fits_f32(f: f64) -> bool {
    f.is_finite() && f.abs() <= f32::MAX as f64
}

fn past_f32(what: &str, v: &Value) -> MeshError {
    malformed(format!("{what} is {}, past a 32-bit float's range", shown_json(v, 60)))
}

/// An optional finite number with a default.
fn number_field(v: Option<&Value>, default: f64, what: &str) -> Result<f64, MeshError> {
    match v {
        None => Ok(default),
        Some(v) => match v.as_f64() {
            Some(f) if fits_f32(f) => Ok(f),
            Some(_) => Err(past_f32(what, v)),
            None => Err(malformed(format!("{what} is {}, which is not a number", shown_json(v, 60)))),
        },
    }
}

/// Morph weights (a mesh's defaults or a node's), each an f32.
fn weights_of(a: &[Value], what: &str) -> Result<Vec<f32>, MeshError> {
    a.iter()
        .map(|v| match v.as_f64() {
            Some(f) if fits_f32(f) => Ok(f as f32),
            Some(_) => Err(past_f32(what, v)),
            None => Err(malformed(format!("{what} are not numbers"))),
        })
        .collect()
}

/// An optional fixed-length array of finite numbers with a default.
fn numbers_field<const N: usize>(v: Option<&Value>, default: [f64; N], what: &str) -> Result<[f64; N], MeshError> {
    let Some(v) = v else { return Ok(default) };
    let a = v
        .as_array()
        .filter(|a| a.len() == N)
        .ok_or_else(|| malformed(format!("{what} is not a {N}-array")))?;
    let mut out = [0.0f64; N];
    for (i, e) in a.iter().enumerate() {
        out[i] = match e.as_f64() {
            Some(f) if fits_f32(f) => f,
            Some(_) => return Err(past_f32(&format!("{what}[{i}]"), e)),
            None => return Err(malformed(format!("{what}[{i}] is not finite"))),
        };
    }
    Ok(out)
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ─── base64 ─────────────────────────────────────────────────────────────────

/// Standard-alphabet base64, tolerating the URL-safe pair and embedded
/// whitespace (some exporters wrap long data URIs).
///
/// Hand-rolled rather than pulled from the crate's `base64` dependency because
/// this module imports nothing outside `super` and `std` — see the isolation
/// note in `super`. It is twenty lines and it is the only thing standing between
/// a hostile `data:` URI and the parser, so it returns `None` rather than
/// guessing at anything it does not recognise.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut padding = 0usize;
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padding += 1;
                continue;
            }
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        // A payload character after '=' means the padding was not terminal.
        if padding > 0 {
            return None;
        }
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // Leftover bits must be zero padding, never a truncated byte.
    if bits >= 6 || (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

/// Standard base64 with padding, for the embedded `.gltf` writer.
fn encode_base64(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

// ─── chunk layer ────────────────────────────────────────────────────────────

/// Split a GLB into its JSON chunk and its optional BIN chunk.
fn split_chunks(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), MeshError> {
    if bytes.len() < HEADER_LEN {
        return Err(not_glb(format!(
            "{} bytes is shorter than the 12-byte GLB header",
            bytes.len()
        )));
    }
    if &bytes[0..4] != MAGIC {
        return Err(not_glb(format!("magic is {:02x?}, expected 'glTF'", &bytes[0..4])));
    }
    let version = u32_le(bytes, 4).ok_or_else(|| not_glb("header is truncated"))?;
    if version != 2 {
        // glTF 1.0 binary is a different container with a different JSON schema;
        // it identifies itself here, so this is a header mismatch and not a
        // feature we chose to skip.
        return Err(not_glb(format!("glTF binary version {version}, only version 2 is supported")));
    }
    let declared = u32_le(bytes, 8).ok_or_else(|| not_glb("header is truncated"))? as usize;
    if declared < HEADER_LEN {
        return Err(malformed(format!(
            "header declares a total length of {declared} bytes, less than the header itself"
        )));
    }
    if declared > bytes.len() {
        return Err(malformed(format!(
            "header declares {declared} bytes but only {} are present (truncated file)",
            bytes.len()
        )));
    }
    // Trailing bytes past the declared length are ignored rather than rejected:
    // a GLB lifted out of a larger stream or a tar member keeps its padding, and
    // the declared length is exactly the authority on where the file ends.
    let end = declared;

    let mut json: Option<&[u8]> = None;
    let mut bin: Option<&[u8]> = None;
    let mut off = HEADER_LEN;
    while off + 8 <= end {
        let len = u32_le(bytes, off).ok_or_else(|| malformed("chunk header is truncated"))? as usize;
        let kind = u32_le(bytes, off + 4).ok_or_else(|| malformed("chunk header is truncated"))?;
        let start = off + 8;
        let stop = start
            .checked_add(len)
            .ok_or_else(|| malformed(format!("chunk at offset {off} declares an absurd length")))?;
        if stop > end {
            return Err(malformed(format!(
                "chunk at offset {off} declares {len} bytes but only {} remain",
                end - start
            )));
        }
        match kind {
            // First-wins: the spec allows exactly one chunk of each type, and a
            // second is corruption we must not silently prefer.
            CHUNK_JSON if json.is_none() => json = Some(&bytes[start..stop]),
            CHUNK_BIN if bin.is_none() => bin = Some(&bytes[start..stop]),
            // "Client implementations MUST ignore chunks with unknown types."
            // That is how vendor-specific sidecar chunks ship, and failing here
            // would reject files every viewer loads.
            _ => {}
        }
        off = stop;
    }
    // Chunk lengths are required to be 4-byte aligned, but nothing downstream
    // depends on that alignment (each chunk is located by its own length), so a
    // sloppy writer's odd chunk is read rather than rejected.
    let json = json.ok_or_else(|| malformed("no JSON chunk in the GLB"))?;
    Ok((json, bin))
}

fn parse_json(chunk: &[u8]) -> Result<Value, MeshError> {
    // The spec pads the JSON chunk with spaces; writers in the wild pad with
    // NULs instead, and serde_json rejects a trailing NUL as trailing garbage.
    // Trimming both is the difference between loading those files and refusing
    // them for a reason that has nothing to do with their content.
    let end = chunk
        .iter()
        .rposition(|b| !matches!(b, b' ' | b'\0' | b'\n' | b'\r' | b'\t'))
        .map_or(0, |i| i + 1);
    // A UTF-8 byte-order mark is legal at the start of a .gltf file and fatal to
    // serde_json; skip it rather than refuse the file over it.
    let start = if chunk.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let doc: Value = serde_json::from_slice(chunk.get(start..end).unwrap_or(&[]))
        .map_err(|e| malformed(format!("JSON chunk does not parse: {e}")))?;
    if !doc.is_object() {
        return Err(malformed("JSON chunk is not a glTF object"));
    }
    Ok(doc)
}

fn check_required_extensions(doc: &Value) -> Result<(), MeshError> {
    let required = match doc.get("extensionsRequired") {
        None => return Ok(()),
        Some(Value::Array(a)) => a,
        // Anything else used to fall straight through to Ok, so the single
        // string `"KHR_draco_mesh_compression"` disabled this guard entirely.
        // Draco had a second check to catch it; meshopt had none.
        Some(other) => {
            return Err(malformed(format!(
                "extensionsRequired is {}, expected an array of extension names",
                truncate(&other.to_string(), 80)
            )))
        }
    };
    for ext in required {
        let name = ext.as_str().unwrap_or("<non-string>");
        if !IGNORABLE_REQUIRED_EXTENSIONS.contains(&name) {
            return Err(unsupported(format!(
                "glTF requires extension {name}, which this converter does not implement"
            )));
        }
    }
    Ok(())
}

fn uses_extension(doc: &Value, name: &str) -> bool {
    ["extensionsUsed", "extensionsRequired"].iter().any(|k| {
        doc.get(*k).and_then(|v| v.as_array()).is_some_and(|a| a.iter().any(|e| e.as_str() == Some(name)))
    })
}

// ─── budget ─────────────────────────────────────────────────────────────────

/// Running total of what the document has decoded, charged before each
/// allocation so a refusal costs nothing.
struct Budget {
    values: Cell<usize>,
    image_bytes: Cell<usize>,
    /// Triangle indices across every primitive read so far.
    indices: Cell<usize>,
}

impl Budget {
    fn new() -> Budget {
        Budget { values: Cell::new(0), image_bytes: Cell::new(0), indices: Cell::new(0) }
    }

    fn charge(&self, values: usize, what: &str) -> Result<(), MeshError> {
        let total = self.values.get().saturating_add(values);
        if total > MAX_DECODED_VALUES {
            return Err(unsupported(format!(
                "decoding {what} would take the document past {MAX_DECODED_VALUES} attribute values; \
                 its accessors describe more data than this converter will hold"
            )));
        }
        self.values.set(total);
        Ok(())
    }

    /// The merged mesh's index ceiling, counted across primitives: checked
    /// per primitive, twelve 12M-index strips each passed it.
    fn charge_indices(&self, indices: usize, at: &str) -> Result<(), MeshError> {
        let total = self.indices.get().saturating_add(indices);
        if total > MAX_MERGED_INDICES {
            return Err(unsupported(format!("the merged mesh passes {MAX_MERGED_INDICES} indices at {at}")));
        }
        self.indices.set(total);
        Ok(())
    }

    fn charge_image(&self, bytes: usize) -> Result<(), MeshError> {
        let total = self.image_bytes.get().saturating_add(bytes);
        if total > MAX_IMAGE_BYTES {
            return Err(unsupported(format!(
                "the document's images exceed {MAX_IMAGE_BYTES} bytes in total"
            )));
        }
        self.image_bytes.set(total);
        Ok(())
    }
}

// ─── accessors ──────────────────────────────────────────────────────────────

/// A sparse accessor's substitution list, already proven in bounds.
struct Sparse<'a> {
    indices: Vec<usize>,
    values: &'a [u8],
}

/// One accessor resolved down to "where the bytes are and how to step them".
struct Accessor<'a> {
    index: usize,
    /// The bytes of its `bufferView` and nothing either side of them, or empty
    /// for a view-less sparse accessor whose base is zeros.
    ///
    /// Holding the whole buffer here and bounding reads against that was how a
    /// view declaring one vertex could serve three: the extra 24 bytes came out
    /// of whatever the next view held — usually the index array — and the caller
    /// got plausible wrong geometry instead of an error.
    data: &'a [u8],
    has_view: bool,
    start: usize,
    stride: usize,
    elem_size: usize,
    comp: u32,
    comp_size: usize,
    normalized: bool,
    ncomp: usize,
    count: usize,
    type_name: String,
    sparse: Option<Sparse<'a>>,
}

/// An accessor's own JSON, checked: its component type and size, its type
/// and component count, `normalized`, and `count`.
struct AccessorHeader<'d> {
    json: &'d Value,
    comp: u32,
    comp_size: usize,
    type_name: String,
    ncomp: usize,
    normalized: bool,
    count: usize,
}

fn type_label(ncomp: usize) -> &'static str {
    match ncomp {
        1 => "SCALAR",
        2 => "VEC2",
        3 => "VEC3",
        4 => "VEC4",
        16 => "MAT4",
        _ => "?",
    }
}

/// One component as a float, under glTF's normalisation rules (§3.11).
fn decode_component(comp: u32, normalized: bool, b: &[u8]) -> f32 {
    match comp {
        COMP_FLOAT => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        COMP_UNSIGNED_BYTE => {
            let v = b[0] as f32;
            if normalized { v / 255.0 } else { v }
        }
        COMP_BYTE => {
            let v = b[0] as i8 as f32;
            if normalized { (v / 127.0).max(-1.0) } else { v }
        }
        COMP_UNSIGNED_SHORT => {
            let v = u16::from_le_bytes([b[0], b[1]]) as f32;
            if normalized { v / 65535.0 } else { v }
        }
        COMP_SHORT => {
            let v = i16::from_le_bytes([b[0], b[1]]) as f32;
            if normalized { (v / 32767.0).max(-1.0) } else { v }
        }
        COMP_UNSIGNED_INT => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
        _ => 0.0,
    }
}

impl Accessor<'_> {
    fn element(&self, i: usize) -> Result<&[u8], MeshError> {
        let off = self
            .start
            .checked_add(i.checked_mul(self.stride).ok_or_else(|| self.overrun(i))?)
            .ok_or_else(|| self.overrun(i))?;
        let stop = off.checked_add(self.elem_size).ok_or_else(|| self.overrun(i))?;
        self.data.get(off..stop).ok_or_else(|| self.overrun(i))
    }

    fn overrun(&self, i: usize) -> MeshError {
        malformed(format!(
            "accessor {} element {i} of {} reads past the end of its {}-byte bufferView",
            self.index,
            self.count,
            self.data.len()
        ))
    }

    fn check_shape(&self, want_ncomp: &[usize], what: &str) -> Result<(), MeshError> {
        if !want_ncomp.contains(&self.ncomp) {
            let wanted: Vec<&str> = want_ncomp.iter().map(|n| type_label(*n)).collect();
            return Err(malformed(format!(
                "{what} accessor {} is {}, expected {}",
                self.index,
                self.type_name,
                wanted.join(" or ")
            )));
        }
        Ok(())
    }

    /// Every component of every element as `f32`, sparse substitutions applied.
    ///
    /// `ok` decides which (componentType, normalized) encodings the semantic
    /// allows. The ones it refuses are `Unsupported` rather than `Malformed`
    /// when they are a real encoding read in the wrong context — a quantized
    /// POSITION without `KHR_mesh_quantization`, say — because reading them as
    /// floats would silently produce garbage coordinates.
    fn read_f32(
        &self,
        want_ncomp: &[usize],
        what: &str,
        ok: impl Fn(u32, bool) -> bool,
        budget: &Budget,
    ) -> Result<Vec<f32>, MeshError> {
        self.check_shape(want_ncomp, what)?;
        if !ok(self.comp, self.normalized) {
            return Err(unsupported(format!(
                "{what} accessor {} uses componentType {}{}; that encoding is not valid for {what} here \
                 (only FLOAT (5126) is supported without an extension that allows it)",
                self.index,
                self.comp,
                if self.normalized { " normalized" } else { "" }
            )));
        }
        let total = self
            .count
            .checked_mul(self.ncomp)
            .ok_or_else(|| malformed(format!("accessor {} size overflows", self.index)))?;
        budget.charge(total, &format!("{what} accessor {}", self.index))?;
        let mut out = vec![0.0f32; total];
        if self.has_view {
            for i in 0..self.count {
                let e = self.element(i)?;
                for c in 0..self.ncomp {
                    out[i * self.ncomp + c] =
                        decode_component(self.comp, self.normalized, &e[c * self.comp_size..]);
                }
            }
        }
        if let Some(sp) = &self.sparse {
            for (k, &target) in sp.indices.iter().enumerate() {
                let base = k * self.elem_size;
                let e = sp.values.get(base..base + self.elem_size).ok_or_else(|| {
                    malformed(format!("accessor {} sparse value {k} is out of bounds", self.index))
                })?;
                for c in 0..self.ncomp {
                    out[target * self.ncomp + c] =
                        decode_component(self.comp, self.normalized, &e[c * self.comp_size..]);
                }
            }
        }
        if let Some(i) = out.iter().position(|v| !v.is_finite()) {
            return Err(malformed(format!(
                "{what} accessor {} element {} is not finite",
                self.index,
                i / self.ncomp
            )));
        }
        Ok(out)
    }

    /// Integer components (indices, joints) with sparse substitutions applied.
    fn read_u32(
        &self,
        want_ncomp: usize,
        what: &str,
        allowed: &[u32],
        budget: &Budget,
    ) -> Result<Vec<u32>, MeshError> {
        self.check_shape(&[want_ncomp], what)?;
        if !allowed.contains(&self.comp) {
            // The spec permits exactly these; anything else is a broken file,
            // not a feature we declined.
            let names: Vec<&str> = allowed
                .iter()
                .map(|c| match *c {
                    COMP_UNSIGNED_BYTE => "UNSIGNED_BYTE",
                    COMP_UNSIGNED_SHORT => "UNSIGNED_SHORT",
                    _ => "UNSIGNED_INT",
                })
                .collect();
            return Err(malformed(format!(
                "{what} accessor {} uses componentType {}; glTF allows only {}",
                self.index,
                self.comp,
                names.join(", ")
            )));
        }
        let total = self
            .count
            .checked_mul(want_ncomp)
            .ok_or_else(|| malformed(format!("accessor {} size overflows", self.index)))?;
        budget.charge(total, &format!("{what} accessor {}", self.index))?;
        let get = |e: &[u8], c: usize| -> u32 {
            let b = &e[c * self.comp_size..];
            match self.comp {
                COMP_UNSIGNED_BYTE => b[0] as u32,
                COMP_UNSIGNED_SHORT => u16::from_le_bytes([b[0], b[1]]) as u32,
                _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            }
        };
        let mut out = vec![0u32; total];
        if self.has_view {
            for i in 0..self.count {
                let e = self.element(i)?;
                for c in 0..want_ncomp {
                    out[i * want_ncomp + c] = get(e, c);
                }
            }
        }
        if let Some(sp) = &self.sparse {
            for (k, &target) in sp.indices.iter().enumerate() {
                let base = k * self.elem_size;
                let e = sp.values.get(base..base + self.elem_size).ok_or_else(|| {
                    malformed(format!("accessor {} sparse value {k} is out of bounds", self.index))
                })?;
                for c in 0..want_ncomp {
                    out[target * want_ncomp + c] = get(e, c);
                }
            }
        }
        Ok(out)
    }

    fn read_indices(&self, budget: &Budget) -> Result<Vec<u32>, MeshError> {
        if self.ncomp != 1 {
            return Err(malformed(format!(
                "index accessor {} is {}, expected SCALAR",
                self.index, self.type_name
            )));
        }
        self.read_u32(1, "index", &[COMP_UNSIGNED_BYTE, COMP_UNSIGNED_SHORT, COMP_UNSIGNED_INT], budget)
    }
}

fn float_only(comp: u32, _normalized: bool) -> bool {
    comp == COMP_FLOAT
}

/// Float, or an unsigned byte/short normalised into [0, 1]: what core glTF
/// allows for TEXCOORD, COLOR and WEIGHTS.
fn float_or_unorm(comp: u32, normalized: bool) -> bool {
    comp == COMP_FLOAT || (normalized && matches!(comp, COMP_UNSIGNED_BYTE | COMP_UNSIGNED_SHORT))
}

/// The attributes `Reader::primitive` reads from a primitive listing `attrs`,
/// walked as it walks them: POSITION, NORMAL, TANGENT and COLOR_0; the
/// TEXCOORD sets from 0 for as long as each is listed, up to MAX_UV_SETS; and
/// the JOINTS/WEIGHTS pairs from 0 for as long as either is listed, up to
/// MAX_INFLUENCE_SETS (an unpaired one it then refuses). Anything else a
/// primitive lists is ignored.
fn read_attributes(attrs: &serde_json::Map<String, Value>) -> Vec<String> {
    let mut read: Vec<String> = ["POSITION", "NORMAL", "TANGENT", "COLOR_0"].into_iter().filter(|n| attrs.contains_key(*n)).map(str::to_string).collect();
    for set in 0..MAX_UV_SETS {
        let key = format!("TEXCOORD_{set}");
        if !attrs.contains_key(&key) {
            break;
        }
        read.push(key);
    }
    for set in 0..MAX_INFLUENCE_SETS {
        let pair = [format!("JOINTS_{set}"), format!("WEIGHTS_{set}")];
        if !pair.iter().any(|k| attrs.contains_key(k)) {
            break;
        }
        read.extend(pair.into_iter().filter(|k| attrs.contains_key(k)));
    }
    read
}

/// A Draco attribute's values laid out as accessor `index` declares its
/// elements, so that they read as the accessor's own data would: the
/// stream's bytes as they are where the component types agree (the usual
/// case), else each integer as the accessor's type, refused past its range,
/// or as a float the way Google's decoder converts one (divided by its
/// type's largest value where the stream marks it normalized, with no clamp
/// to −1 for a signed normalized value, as Google's
/// `ConvertComponentValue`). A float the
/// accessor declares an integer is refused: no conversion keeps its value.
///
/// The widened copy is bounded by draco.rs's ceiling: at most
/// 4 × [`draco::MAX_DECODED_BYTES`] a primitive (a byte attribute read as
/// `f32`), transient, and not charged to a budget first (Ruling B50).
fn draco_values<'v>(values: &'v draco::DecodedAttribute, h: &AccessorHeader<'_>, index: usize, at: &str) -> Result<Cow<'v, [u8]>, MeshError> {
    use draco::ValueKind as K;
    if values.components != h.ncomp {
        return Err(malformed(format!("{at}: accessor {index} is {}, but its Draco attribute has {} components", h.type_name, values.components)));
    }
    let target = match h.comp {
        COMP_BYTE => K::I8,
        COMP_UNSIGNED_BYTE => K::U8,
        COMP_SHORT => K::I16,
        COMP_UNSIGNED_SHORT => K::U16,
        COMP_UNSIGNED_INT => K::U32,
        _ => K::F32,
    };
    if values.kind == target {
        return Ok(Cow::Borrowed(&values.bytes));
    }
    let size = values.kind.size();
    let mut out = Vec::with_capacity(values.bytes.len() / size * target.size());
    for c in values.bytes.chunks_exact(size) {
        let (v, largest): (i64, f32) = match values.kind {
            K::I8 => (i64::from(c[0] as i8), f32::from(i8::MAX)),
            K::U8 => (i64::from(c[0]), f32::from(u8::MAX)),
            K::I16 => (i64::from(i16::from_le_bytes([c[0], c[1]])), f32::from(i16::MAX)),
            K::U16 => (i64::from(u16::from_le_bytes([c[0], c[1]])), f32::from(u16::MAX)),
            K::I32 => (i64::from(i32::from_le_bytes([c[0], c[1], c[2], c[3]])), i32::MAX as f32),
            K::U32 => (i64::from(u32::from_le_bytes([c[0], c[1], c[2], c[3]])), u32::MAX as f32),
            K::F32 => {
                return Err(malformed(format!(
                    "{at}: its Draco attribute decodes to 32-bit floats, which accessor {index}'s componentType {} can't hold",
                    h.comp
                )))
            }
        };
        let fits = match target {
            K::F32 => {
                let f = if values.normalized { v as f32 / largest } else { v as f32 };
                out.extend_from_slice(&f.to_le_bytes());
                true
            }
            K::I8 => i8::try_from(v).map(|x| out.extend_from_slice(&x.to_le_bytes())).is_ok(),
            K::U8 => u8::try_from(v).map(|x| out.push(x)).is_ok(),
            K::I16 => i16::try_from(v).map(|x| out.extend_from_slice(&x.to_le_bytes())).is_ok(),
            K::U16 => u16::try_from(v).map(|x| out.extend_from_slice(&x.to_le_bytes())).is_ok(),
            K::I32 => i32::try_from(v).map(|x| out.extend_from_slice(&x.to_le_bytes())).is_ok(),
            K::U32 => u32::try_from(v).map(|x| out.extend_from_slice(&x.to_le_bytes())).is_ok(),
        };
        if !fits {
            return Err(malformed(format!("{at}: its Draco attribute holds {v}, past accessor {index}'s componentType {}", h.comp)));
        }
    }
    Ok(Cow::Owned(out))
}

// ─── document ───────────────────────────────────────────────────────────────

struct Gltf<'a> {
    doc: Value,
    /// Resolved lazily in spirit but eagerly in fact: a buffer that cannot be
    /// resolved stores its error instead of raising it, because a file may
    /// carry an external-URI buffer that only something we never read uses —
    /// and refusing the whole file over a buffer we never read would be wrong.
    buffers: Vec<Result<Cow<'a, [u8]>, MeshError>>,
    /// Each meshopt-compressed bufferView, by index: decoded the first time
    /// an accessor, a sparse list or an image reads it, then sliced by every
    /// later reader, so a view that accessors share is decoded and charged
    /// to the budget once (Ruling B11). A failed decode keeps its error and
    /// returns it again, as `buffers` does. Only views that carry an
    /// extension get a slot, so a file of a million plain views costs
    /// nothing here.
    decoded: HashMap<usize, OnceCell<Result<Vec<u8>, MeshError>>>,
    /// What decoding those views found to warn about; the read reports it.
    notes: RefCell<Vec<String>>,
    quantized: bool,
    budget: Budget,
}

impl<'a> Gltf<'a> {
    fn new(doc: Value, bin: Option<&'a [u8]>, sidecars: &dyn Sidecars) -> Self {
        let declared = doc.get("buffers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let buffers = declared
            .iter()
            .enumerate()
            .map(|(i, b)| match b.get("uri").and_then(|u| u.as_str()) {
                // No URI means "the GLB's own BIN chunk". The declared
                // `byteLength` is deliberately not used to trim the chunk: some
                // writers round it and some understate it, and the per-element
                // bounds checks are what actually keep reads in range.
                None => match bin {
                    Some(b) => Ok(Cow::Borrowed(b)),
                    None => Err(malformed(format!(
                        "buffer {i} has no uri but the GLB has no BIN chunk"
                    ))),
                },
                Some(uri) if package::is_data_uri(uri) => {
                    decode_data_uri(uri, &format!("buffer {i}")).map(|(_, b)| Cow::Owned(b))
                }
                Some(uri) => match package::normalize_relative(uri).and_then(|p| sidecars.get(&p)) {
                    Some(bytes) => Ok(Cow::Owned(bytes)),
                    None => Err(unsupported(format!(
                        "buffer {i} points at the external file {:?}, which was not supplied; \
                         conversion runs without a filesystem, so only embedded buffers and \
                         files handed over with the model work",
                        truncate(uri, 120)
                    ))),
                },
            })
            .collect();
        let quantized = uses_extension(&doc, "KHR_mesh_quantization");
        let decoded = doc
            .get("bufferViews")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, view)| meshopt_extensions(view).next().is_some())
            .map(|(i, _)| (i, OnceCell::new()))
            .collect();
        Gltf { doc, buffers, decoded, notes: RefCell::new(Vec::new()), quantized, budget: Budget::new() }
    }

    fn array(&self, key: &str) -> &[Value] {
        self.doc.get(key).and_then(|v| v.as_array()).map_or(&[], |v| v.as_slice())
    }

    fn buffer(&self, i: usize) -> Result<&[u8], MeshError> {
        match self.buffers.get(i) {
            Some(Ok(b)) => Ok(b),
            Some(Err(e)) => Err(e.clone()),
            None => Err(malformed(format!(
                "buffer index {i} out of range ({} buffers)",
                self.buffers.len()
            ))),
        }
    }

    /// The bytes of one bufferView, bounded by its own declared extent: a
    /// meshopt view's decoded stream, or a slice of its buffer.
    fn view_bytes(&self, view_index: usize) -> Result<(&[u8], &Value), MeshError> {
        let view = self
            .array("bufferViews")
            .get(view_index)
            .ok_or_else(|| malformed(format!("bufferView index {view_index} out of range")))?;
        if let Some(slot) = self.decoded.get(&view_index) {
            // A meshopt view's own `buffer` range is its fallback: absent, a
            // placeholder, or the plain data. It is never read, not even when
            // it is the BIN chunk and the extension is only used, a file that
            // needn't declare the extension required. The stream is decoded
            // instead, once, whatever reads the view.
            let decoded = slot.get_or_init(|| self.decode_meshopt_view(view_index, view)).as_ref().map_err(MeshError::clone)?;
            return Ok((decoded.as_slice(), view));
        }
        let buffer_index = view
            .get("buffer")
            .and_then(as_usize)
            .ok_or_else(|| malformed(format!("bufferView {view_index} has no buffer")))?;
        let buffer = self.buffer(buffer_index)?;
        let view_offset =
            index_field(view.get("byteOffset"), &format!("bufferView {view_index} byteOffset"))?
                .unwrap_or(0);
        // `byteLength` is required by the spec and is the only thing that says
        // where this view stops. Defaulting it to "the rest of the buffer" is
        // what let an accessor read its neighbour's bytes, so a view without one
        // is a file we cannot bound rather than one we guess at.
        let view_len =
            index_field(view.get("byteLength"), &format!("bufferView {view_index} byteLength"))?
                .ok_or_else(|| malformed(format!("bufferView {view_index} has no byteLength")))?;
        let view_end = view_offset
            .checked_add(view_len)
            .ok_or_else(|| malformed(format!("bufferView {view_index} has an absurd byteOffset")))?;
        let data = buffer.get(view_offset..view_end).ok_or_else(|| {
            malformed(format!(
                "bufferView {view_index} spans bytes {view_offset}..{view_end} of a {}-byte buffer",
                buffer.len()
            ))
        })?;
        Ok((data, view))
    }

    /// One meshopt view's stream, decoded by `meshopt::decode_view` into the
    /// `count × byteStride` bytes it stands for. The extension's JSON is
    /// checked first, and the declared size charged to the budget, so that a
    /// short stream's `count` is never an allocation request (Ruling B1).
    fn decode_meshopt_view(&self, view_index: usize, view: &Value) -> Result<Vec<u8>, MeshError> {
        let at = format!("bufferView {view_index}");
        let found: Vec<(&str, &Value)> = meshopt_extensions(view).collect();
        let [(ext, e)] = found[..] else {
            return Err(malformed(format!("{at} carries both meshopt extensions, which KHR_meshopt_compression forbids")));
        };
        let field = |name: &str| index_field(e.get(name), &format!("{at} {ext} {name}"));
        let needed = |name: &str| field(name)?.ok_or_else(|| malformed(format!("{at} {ext} has no {name}")));
        let buffer = needed("buffer")?;
        let offset = field("byteOffset")?.unwrap_or(0);
        let length = needed("byteLength")?;
        let stride = needed("byteStride")?;
        // EXT states that a view's own byteStride, when it has one, is the
        // extension's. A mismatch describes nothing coherent (the stream
        // decodes count × the extension's, and the accessors would step
        // through it at the view's), so it is refused, not guessed at
        // (final review, Minor 1).
        if let Some(own) = index_field(view.get("byteStride"), &format!("{at} byteStride"))? {
            if own != stride {
                return Err(malformed(format!(
                    "{at} byteStride is {own}, but its {ext} byteStride {stride}: the two must agree, and this reader won't guess which the data is laid out at"
                )));
            }
        }
        let count = needed("count")?;
        let mode = match e.get("mode").and_then(Value::as_str) {
            Some("ATTRIBUTES") => meshopt::Mode::Attributes,
            Some("TRIANGLES") => meshopt::Mode::Triangles,
            Some("INDICES") => meshopt::Mode::Indices,
            _ => {
                let shown = e.get("mode").map_or_else(|| "absent".to_string(), |m| shown_json(m, 60));
                return Err(malformed(format!("{at} {ext} mode is {shown}, not ATTRIBUTES, TRIANGLES or INDICES")));
            }
        };
        let filter = match e.get("filter").map(Value::as_str) {
            None | Some(Some("NONE")) => meshopt::Filter::None,
            Some(Some("OCTAHEDRAL")) => meshopt::Filter::Octahedral,
            Some(Some("QUATERNION")) => meshopt::Filter::Quaternion,
            Some(Some("EXPONENTIAL")) => meshopt::Filter::Exponential,
            Some(Some("COLOR")) => meshopt::Filter::Color,
            Some(_) => {
                let shown = shown_json(&e["filter"], 60);
                return Err(malformed(format!("{at} {ext} filter is {shown}, not NONE, OCTAHEDRAL, QUATERNION, EXPONENTIAL or COLOR")));
            }
        };
        let decoded_len = count
            .checked_mul(stride)
            .ok_or_else(|| malformed(format!("{at} {ext} count {count} × byteStride {stride} overflows")))?;
        let view_len = index_field(view.get("byteLength"), &format!("{at} byteLength"))?
            .ok_or_else(|| malformed(format!("{at} has no byteLength")))?;
        if view_len != decoded_len {
            return Err(malformed(format!(
                "{at} byteLength is {view_len}, but its {ext} stream decodes to count {count} × byteStride {stride} = {decoded_len} bytes"
            )));
        }
        // A missing or broken stream buffer is named under the view and the
        // extension it was read for (final review, Minor 10).
        let source = self.buffer(buffer).map_err(|e| match e {
            MeshError::Unsupported(d) => unsupported(format!("{at} {ext}: {d}")),
            MeshError::Malformed(d) => malformed(format!("{at} {ext}: {d}")),
            other => other,
        })?;
        let stream = offset.checked_add(length).and_then(|end| source.get(offset..end)).ok_or_else(|| {
            malformed(format!("{at} {ext} spans bytes {offset}..{} of a {}-byte buffer", offset.saturating_add(length), source.len()))
        })?;
        // Before the decoder sizes its output from `count`: a stream of a few
        // bytes can declare any count.
        self.budget.charge(decoded_len.div_ceil(4), &format!("{ext} {at} ({count} × {stride} bytes)"))?;
        let decoded = meshopt::decode_view(stream, count, stride, mode, filter).map_err(|err| match err {
            MeshError::Malformed(d) => malformed(format!("{at} ({ext}): {d}")),
            other => other,
        })?;
        if ext == "EXT_meshopt_compression" {
            // EXT allows attribute format v0 alone and has no COLOR filter;
            // KHR adds both. A stream says which format it is, so such a view
            // decodes, and the read warns rather than refusing a file the
            // vendor has billed for (Ruling B9).
            let mut notes = self.notes.borrow_mut();
            if mode == meshopt::Mode::Attributes && stream.first() == Some(&0xa1) {
                notes.push(format!(
                    "{at} is EXT_meshopt_compression with attribute format v1 (header 0xa1), which only KHR_meshopt_compression \
                     defines; it was decoded as the stream describes itself"
                ));
            }
            if filter == meshopt::Filter::Color {
                notes.push(format!(
                    "{at} is EXT_meshopt_compression with the COLOR filter, which only KHR_meshopt_compression defines; it was \
                     decoded as KHR_meshopt_compression's COLOR"
                ));
            }
        }
        Ok(decoded)
    }

    /// What accessor `index`'s JSON says of its elements, before any of
    /// their bytes are found: shared by [`Gltf::accessor`] and the Draco
    /// path, which reads an accessor's elements from the decoded stream.
    fn accessor_header(&self, index: usize) -> Result<AccessorHeader<'_>, MeshError> {
        let a = self
            .array("accessors")
            .get(index)
            .ok_or_else(|| malformed(format!("accessor index {index} out of range")))?;
        let comp = a
            .get("componentType")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| malformed(format!("accessor {index} has no componentType")))?
            as u32;
        let comp_size = match comp {
            COMP_BYTE | COMP_UNSIGNED_BYTE => 1usize,
            COMP_SHORT | COMP_UNSIGNED_SHORT => 2,
            COMP_UNSIGNED_INT | COMP_FLOAT => 4,
            other => {
                return Err(malformed(format!("accessor {index} has componentType {other}")))
            }
        };
        let type_name = a
            .get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| malformed(format!("accessor {index} has no type")))?
            .to_string();
        let ncomp = match type_name.as_str() {
            "SCALAR" => 1usize,
            "VEC2" => 2,
            "VEC3" => 3,
            "VEC4" => 4,
            // Inverse bind matrices are the one matrix accessor a scene needs,
            // and as FLOAT they have no column padding to decode.
            "MAT4" if comp == COMP_FLOAT => 16,
            "MAT2" | "MAT3" | "MAT4" => {
                return Err(unsupported(format!(
                    "accessor {index} is {type_name} of componentType {comp}; only FLOAT MAT4 \
                     (inverse bind matrices) appears in a scene this converter models"
                )))
            }
            other => return Err(malformed(format!("accessor {index} has type {other:?}"))),
        };
        let normalized = match a.get("normalized") {
            None => false,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| malformed(format!("accessor {index} normalized is not a boolean")))?,
        };
        let count = a
            .get("count")
            .and_then(as_usize)
            .ok_or_else(|| malformed(format!("accessor {index} has no count")))?;
        Ok(AccessorHeader { json: a, comp, comp_size, type_name, ncomp, normalized, count })
    }

    fn accessor(&self, index: usize) -> Result<Accessor<'_>, MeshError> {
        let AccessorHeader { json: a, comp, comp_size, type_name, ncomp, normalized, count } = self.accessor_header(index)?;
        let elem_size = ncomp * comp_size;
        let acc_offset =
            index_field(a.get("byteOffset"), &format!("accessor {index} byteOffset"))?.unwrap_or(0);
        let sparse = match a.get("sparse") {
            None => None,
            Some(sp) => Some(self.sparse(index, sp, count, elem_size)?),
        };

        let Some(view_index) =
            index_field(a.get("bufferView"), &format!("accessor {index} bufferView"))?
        else {
            if sparse.is_some() {
                // Zeros overlaid with a sparse patch: the standard way to ship a
                // morph target that moves a handful of vertices. The base costs
                // nothing to "read"; what it costs to allocate is charged to the
                // budget at decode time, so `count` cannot become an allocation
                // request on its own.
                return Ok(Accessor {
                    index,
                    data: &[],
                    has_view: false,
                    start: 0,
                    stride: elem_size,
                    elem_size,
                    comp,
                    comp_size,
                    normalized,
                    ncomp,
                    count,
                    type_name,
                    sparse,
                });
            }
            // Spec §3.6.2.1: a view-less accessor reads as zeros (Blender
            // writes an empty shape key so). Its `count` is backed by no bytes,
            // so the zeros are charged to the budget at decode time like a
            // sparse base, never sized from `count` alone (a 244-byte file once
            // asked for 48 GB that way).
            return Ok(Accessor {
                index,
                data: &[],
                has_view: false,
                start: 0,
                stride: elem_size,
                elem_size,
                comp,
                comp_size,
                normalized,
                ncomp,
                count,
                type_name,
                sparse: None,
            });
        };
        let (data, view) = self.view_bytes(view_index)?;
        // Interleaved vertex buffers are the norm, not the exception: one view
        // holds POSITION, NORMAL and TEXCOORD_0 side by side and each accessor
        // steps by the shared stride from its own byteOffset.
        let stride =
            match index_field(view.get("byteStride"), &format!("bufferView {view_index} byteStride"))?
            {
                Some(0) | None => elem_size,
                Some(s) if s < elem_size => {
                    return Err(malformed(format!(
                        "bufferView {view_index} has byteStride {s}, smaller than the \
                         {elem_size}-byte element of accessor {index}"
                    )))
                }
                Some(s) => s,
            };
        // The spec's own inequality, checked here and not per element, because
        // the read loops size a Vec from `count` before the first element is
        // fetched: proving the bytes exist is what keeps `count` from being an
        // allocation request.
        let span = match count.checked_sub(1) {
            None => 0,
            Some(last) => last
                .checked_mul(stride)
                .and_then(|o| o.checked_add(elem_size))
                .ok_or_else(|| {
                    malformed(format!("accessor {index} declares an unrepresentable {count} elements"))
                })?,
        };
        let need = acc_offset
            .checked_add(span)
            .ok_or_else(|| malformed(format!("accessor {index} has an absurd byteOffset")))?;
        if need > data.len() {
            return Err(malformed(format!(
                "accessor {index} reads past the end of bufferView {view_index}: {count} elements \
                 of {elem_size} bytes at stride {stride} from offset {acc_offset} need {need} \
                 bytes, and the view declares {}",
                data.len()
            )));
        }
        Ok(Accessor {
            index,
            data,
            has_view: true,
            start: acc_offset,
            stride,
            elem_size,
            comp,
            comp_size,
            normalized,
            ncomp,
            count,
            type_name,
            sparse,
        })
    }

    /// A Draco primitive's stream, decoded by `draco.rs` within the absolute
    /// [`DRACO_LIMITS`], with the attributes its extension maps that this
    /// reader reads ([`read_attributes`]). The stream
    /// is its `bufferView`'s bytes, so a meshopt-compressed view is
    /// meshopt-decoded first. The extension's `attributes` are a subset of
    /// the primitive's (README :109). A name of its own that this reader
    /// would read is refused: no accessor gives its componentType or
    /// normalized, so there is nothing to read it by. One this reader
    /// wouldn't read is skipped with a warning, as three.js ignores such a
    /// mapping (Ruling B49).
    fn decode_draco(&self, ext: &Value, attrs: &serde_json::Map<String, Value>, at: &str) -> Result<draco::DecodedPrimitive, MeshError> {
        let view = index_field(ext.get("bufferView"), &format!("{at} {DRACO_EXTENSION} bufferView"))?
            .ok_or_else(|| malformed(format!("{at}'s {DRACO_EXTENSION} has no bufferView")))?;
        let mapped = ext
            .get("attributes")
            .and_then(Value::as_object)
            .ok_or_else(|| malformed(format!("{at}'s {DRACO_EXTENSION} has no attributes object")))?;
        // Only what `primitive` reads is decoded and copied: a mapped
        // attribute this reader ignores, as it ignores the uncompressed one,
        // costs nothing (Ruling B37c). Every name the extension maps is still
        // checked.
        let read = read_attributes(attrs);
        // The names the extension maps and the primitive doesn't list
        // (Ruling B49), judged in one pass: would this reader read the
        // semantic, had the primitive listed it? `read_attributes` over the
        // primitive's attributes plus every such name walks them as the
        // read does, so it answers for the sets and pairs too. One pass and
        // one warning, so a file of many such names costs attributes plus
        // mappings, not their product (the final wave's re-review, N1).
        let unlisted: Vec<&String> = mapped.keys().filter(|n| !attrs.contains_key(*n)).collect();
        if !unlisted.is_empty() {
            let mut listed = attrs.clone();
            for n in &unlisted {
                listed.insert((*n).clone(), Value::Null);
            }
            let would_read = read_attributes(&listed);
            if let Some(name) = unlisted.iter().find(|n| would_read.contains(n)) {
                return Err(malformed(format!(
                    "{at}'s {DRACO_EXTENSION} maps {name}, which the primitive's attributes don't list, and which this reader would read: no accessor gives its componentType"
                )));
            }
            let shown = unlisted.iter().take(4).map(|n| n.as_str()).collect::<Vec<_>>().join(", ");
            let more = if unlisted.len() > 4 || shown.len() > 60 { ", …" } else { "" };
            self.notes.borrow_mut().push(format!(
                "{at}'s {DRACO_EXTENSION} maps {} attribute(s) the primitive's attributes don't list and this reader doesn't read ({}{more}); the mappings were skipped",
                unlisted.len(),
                truncate(&shown, 60)
            ));
        }
        let mut names = Vec::with_capacity(mapped.len());
        for (name, id) in mapped {
            if !attrs.contains_key(name) {
                continue;
            }
            let id = index_field(Some(id), &format!("{at} {DRACO_EXTENSION} {name}"))?
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| malformed(format!("{at}'s {DRACO_EXTENSION} {name} is no Draco attribute id")))?;
            if read.contains(name) {
                names.push((name.clone(), id));
            }
        }
        let (stream, _) = self.view_bytes(view)?;
        draco::decode_primitive(stream, &names, DRACO_LIMITS).map_err(|e| match e {
            MeshError::Malformed(d) => malformed(format!("{at} ({DRACO_EXTENSION}): {d}")),
            MeshError::Unsupported(d) => unsupported(format!("{at} ({DRACO_EXTENSION}): {d}")),
            other => other,
        })
    }

    /// Accessor `index`, the primitive's `semantic`, handed to `read`. Where
    /// the primitive's Draco extension maps the semantic, its elements are
    /// the stream's decoded values, and the accessor's own bufferView and
    /// byteOffset are ignored (README :132), its data then charged and
    /// checked exactly as uncompressed data is. Anything else is read as
    /// usual (:133), which in a Draco primitive needs data of its own: a
    /// view-less accessor there is one the extension forgot, and reading it
    /// as zeros would hand the semantic values no file gave it.
    fn read_attribute<T>(
        &self,
        index: usize,
        semantic: &str,
        draco: Option<&draco::DecodedPrimitive>,
        at: &str,
        read: impl FnOnce(&Accessor<'_>) -> Result<T, MeshError>,
    ) -> Result<T, MeshError> {
        let Some(d) = draco else { return read(&self.accessor(index)?) };
        let Some((_, values)) = d.attributes.iter().find(|(name, _)| name == semantic) else {
            if self.array("accessors").get(index).is_some_and(|a| a.get("bufferView").is_none() && a.get("sparse").is_none()) {
                return Err(malformed(format!(
                    "{at} lists {semantic} (accessor {index}), which its {DRACO_EXTENSION} attributes don't map and which has no data of its own"
                )));
            }
            return read(&self.accessor(index)?);
        };
        let h = self.accessor_header(index)?;
        if h.json.get("sparse").is_some() {
            return Err(malformed(format!("{at} {semantic} accessor {index} is sparse, which a {DRACO_EXTENSION} attribute can't be")));
        }
        let data = draco_values(values, &h, index, &format!("{at} {semantic}"))?;
        let elem_size = h.ncomp * h.comp_size;
        read(&Accessor {
            index,
            data: &data,
            has_view: true,
            start: 0,
            stride: elem_size,
            elem_size,
            comp: h.comp,
            comp_size: h.comp_size,
            normalized: h.normalized,
            ncomp: h.ncomp,
            count: values.count(),
            type_name: h.type_name,
            sparse: None,
        })
    }

    /// Resolve and bound a sparse substitution list (§3.6.2.3).
    fn sparse(&self, index: usize, sp: &Value, count: usize, elem_size: usize) -> Result<Sparse<'_>, MeshError> {
        let n = sp
            .get("count")
            .and_then(as_usize)
            .ok_or_else(|| malformed(format!("accessor {index} sparse has no count")))?;
        if n == 0 || n > count {
            return Err(malformed(format!(
                "accessor {index} sparse count {n} is not within 1..={count}"
            )));
        }
        let idx = sp
            .get("indices")
            .ok_or_else(|| malformed(format!("accessor {index} sparse has no indices")))?;
        let vals = sp
            .get("values")
            .ok_or_else(|| malformed(format!("accessor {index} sparse has no values")))?;
        let idx_view = index_field(idx.get("bufferView"), &format!("accessor {index} sparse indices bufferView"))?
            .ok_or_else(|| malformed(format!("accessor {index} sparse indices have no bufferView")))?;
        let idx_off = index_field(idx.get("byteOffset"), &format!("accessor {index} sparse indices byteOffset"))?
            .unwrap_or(0);
        let idx_comp = idx
            .get("componentType")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| malformed(format!("accessor {index} sparse indices have no componentType")))?
            as u32;
        let idx_size = match idx_comp {
            COMP_UNSIGNED_BYTE => 1,
            COMP_UNSIGNED_SHORT => 2,
            COMP_UNSIGNED_INT => 4,
            other => {
                return Err(malformed(format!(
                    "accessor {index} sparse indices use componentType {other}"
                )))
            }
        };
        self.budget.charge(n, &format!("accessor {index} sparse indices"))?;
        let (idx_data, _) = self.view_bytes(idx_view)?;
        let idx_bytes = n
            .checked_mul(idx_size)
            .and_then(|b| b.checked_add(idx_off))
            .filter(|end| *end <= idx_data.len())
            .ok_or_else(|| malformed(format!("accessor {index} sparse indices read past their bufferView")))?;
        let idx_bytes = &idx_data[idx_off..idx_bytes];
        let mut indices = Vec::with_capacity(n);
        let mut prev: Option<usize> = None;
        for k in 0..n {
            let b = &idx_bytes[k * idx_size..];
            let v = match idx_size {
                1 => b[0] as usize,
                2 => u16::from_le_bytes([b[0], b[1]]) as usize,
                _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize,
            };
            // Strictly increasing is a spec requirement, and it is what makes
            // "each element patched at most once" true.
            if v >= count || prev.is_some_and(|p| v <= p) {
                return Err(malformed(format!(
                    "accessor {index} sparse index {v} is out of range or out of order"
                )));
            }
            prev = Some(v);
            indices.push(v);
        }
        let val_view = index_field(vals.get("bufferView"), &format!("accessor {index} sparse values bufferView"))?
            .ok_or_else(|| malformed(format!("accessor {index} sparse values have no bufferView")))?;
        let val_off = index_field(vals.get("byteOffset"), &format!("accessor {index} sparse values byteOffset"))?
            .unwrap_or(0);
        let (val_data, _) = self.view_bytes(val_view)?;
        let val_end = n
            .checked_mul(elem_size)
            .and_then(|b| b.checked_add(val_off))
            .filter(|end| *end <= val_data.len())
            .ok_or_else(|| malformed(format!("accessor {index} sparse values read past their bufferView")))?;
        Ok(Sparse { indices, values: &val_data[val_off..val_end] })
    }
}

fn decode_data_uri(uri: &str, what: &str) -> Result<(String, Vec<u8>), MeshError> {
    let comma = uri
        .find(',')
        .ok_or_else(|| malformed(format!("{what} has a data: uri with no comma")))?;
    let meta = &uri[..comma];
    if !meta.to_ascii_lowercase().contains(";base64") {
        // Percent-encoded data URIs are legal and essentially never emitted for
        // binary content; decoding them is a second decoder's worth of attack
        // surface for no vendor.
        return Err(unsupported(format!("{what} uses a non-base64 data: uri")));
    }
    let mime = meta.get(5..).unwrap_or("").split(';').next().unwrap_or("").to_string();
    let payload = &uri[comma + 1..];
    decode_base64(payload)
        .map(|b| (mime, b))
        .ok_or_else(|| malformed(format!("{what} has an invalid base64 payload")))
}

// ─── node transforms ────────────────────────────────────────────────────────

fn node_transform(node: &Value, index: usize) -> Result<Transform, MeshError> {
    // `matrix` and TRS are mutually exclusive per spec; when a writer emits
    // both, `matrix` is the one a conforming loader would have used.
    if let Some(m) = node.get("matrix") {
        let a = m
            .as_array()
            .ok_or_else(|| malformed(format!("node {index} matrix is not an array")))?;
        if a.len() != 16 {
            return Err(malformed(format!(
                "node {index} matrix has {} entries, expected 16",
                a.len()
            )));
        }
        let mut out = [0.0f64; 16];
        for (i, v) in a.iter().enumerate() {
            out[i] = match v.as_f64() {
                Some(f) if fits_f32(f) => f,
                Some(_) => return Err(past_f32(&format!("node {index} matrix entry {i}"), v)),
                None => return Err(malformed(format!("node {index} matrix entry {i} is not finite"))),
            };
        }
        return Ok(Transform::from_matrix(out));
    }
    let t = numbers_field::<3>(node.get("translation"), [0.0; 3], &format!("node {index} translation"))?;
    let s = numbers_field::<3>(node.get("scale"), [1.0; 3], &format!("node {index} scale"))?;
    let q = match node.get("rotation") {
        None => math::QUAT_IDENTITY,
        Some(_) => {
            let q = numbers_field::<4>(node.get("rotation"), [0.0; 4], &format!("node {index} rotation"))?;
            // Exporters routinely emit quaternions a few ULPs off unit length;
            // renormalising absorbs that, while a genuinely zero quaternion has
            // no rotation to recover and is reported rather than silently
            // treated as identity.
            let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            if len <= 0.0 {
                return Err(malformed(format!("node {index} has a zero-length rotation quaternion")));
            }
            [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
        }
    };
    Ok(Transform::Trs { translation: t, rotation: q, scale: s })
}

// ─── primitive topology ─────────────────────────────────────────────────────

/// `TRIANGLE_STRIP` (mode 5) as the triangle list it stands for.
///
/// Every odd-numbered triangle swaps its first two vertices, and that swap is
/// the entire difficulty: a strip alternates which way round its corners are
/// written, so expanding it naively leaves every second face pointing inward and
/// the model half-invisible under back-face culling.
fn strip_to_triangles(seq: &[u32], where_: &str) -> Result<Vec<u32>, MeshError> {
    if seq.len() < 3 {
        return Err(malformed(format!(
            "{where_} is a triangle strip of {} vertices; a strip needs at least 3",
            seq.len()
        )));
    }
    let mut out = Vec::with_capacity((seq.len() - 2) * 3);
    for (t, w) in seq.windows(3).enumerate() {
        if t.is_multiple_of(2) {
            out.extend_from_slice(&[w[0], w[1], w[2]]);
        } else {
            out.extend_from_slice(&[w[1], w[0], w[2]]);
        }
    }
    Ok(out)
}

/// `TRIANGLE_FAN` (mode 6) as the triangle list it stands for: every triangle
/// hangs off the first vertex, in order, so the winding needs no fixing up.
fn fan_to_triangles(seq: &[u32], where_: &str) -> Result<Vec<u32>, MeshError> {
    if seq.len() < 3 {
        return Err(malformed(format!(
            "{where_} is a triangle fan of {} vertices; a fan needs at least 3",
            seq.len()
        )));
    }
    let hub = seq[0];
    let mut out = Vec::with_capacity((seq.len() - 2) * 3);
    for w in seq[1..].windows(2) {
        out.extend_from_slice(&[hub, w[0], w[1]]);
    }
    Ok(out)
}

/// `primitive.mode` 0: every index one point.
const MODE_POINTS: u64 = 0;
/// Mode 1: every two indices one segment.
const MODE_LINES: u64 = 1;
/// Mode 2: each vertex joined to the next, and the last to the first.
const MODE_LINE_LOOP: u64 = 2;
/// Mode 3: each vertex joined to the next.
const MODE_LINE_STRIP: u64 = 3;

/// `LINE_STRIP` (mode 3) as the line list it stands for: n vertices are n - 1
/// segments, each from a vertex to the one after it.
fn line_strip_to_lines(seq: &[u32], where_: &str) -> Result<Vec<u32>, MeshError> {
    if seq.len() < 2 {
        return Err(malformed(format!(
            "{where_} is a line strip of {} vertices; a strip needs at least 2",
            seq.len()
        )));
    }
    let mut out = Vec::with_capacity((seq.len() - 1) * 2);
    for w in seq.windows(2) {
        out.extend_from_slice(&[w[0], w[1]]);
    }
    Ok(out)
}

/// `LINE_LOOP` (mode 2) as the line list it stands for: a strip, closed by a
/// segment from the last vertex back to the first, so n vertices are n
/// segments.
fn line_loop_to_lines(seq: &[u32], where_: &str) -> Result<Vec<u32>, MeshError> {
    if seq.len() < 2 {
        return Err(malformed(format!(
            "{where_} is a line loop of {} vertices; a loop needs at least 2",
            seq.len()
        )));
    }
    let mut out = Vec::with_capacity(seq.len() * 2);
    for w in seq.windows(2) {
        out.extend_from_slice(&[w[0], w[1]]);
    }
    out.extend_from_slice(&[seq[seq.len() - 1], seq[0]]);
    Ok(out)
}

fn chunk3(v: Vec<f32>) -> Vec<[f32; 3]> {
    v.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect()
}

fn chunk2(v: Vec<f32>) -> Vec<[f32; 2]> {
    v.chunks_exact(2).map(|c| [c[0], c[1]]).collect()
}

fn chunk4(v: Vec<f32>) -> Vec<[f32; 4]> {
    v.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect()
}

// ─── document → scene ───────────────────────────────────────────────────────

/// Read state: glTF indices mapped onto IR indices, filled on first use so
/// that only what the selected scene reaches is decoded.
struct Reader<'g, 'a> {
    g: &'g Gltf<'a>,
    scene: Scene,
    warnings: Vec<String>,
    mesh_map: HashMap<usize, usize>,
    material_map: HashMap<usize, usize>,
    texture_map: HashMap<usize, Option<usize>>,
    image_map: HashMap<usize, Option<usize>>,
    skin_map: HashMap<usize, usize>,
    node_map: HashMap<usize, usize>,
    /// The file's cameras and `KHR_lights_punctual` lights by index: the
    /// scene's index of each one a node placed, or `None` for one dropped
    /// with a warning.
    camera_map: HashMap<usize, Option<usize>>,
    light_map: HashMap<usize, Option<usize>>,
    sidecars: &'g dyn Sidecars,
    total_vertices: usize,
    /// A layer was dropped for a texture the document cannot give, and what
    /// its other maps read may be referenced by nothing.
    unreferenced: bool,
}

impl<'g, 'a> Reader<'g, 'a> {
    fn mesh(&mut self, gi: usize) -> Result<usize, MeshError> {
        if let Some(&i) = self.mesh_map.get(&gi) {
            return Ok(i);
        }
        let m = self
            .g
            .array("meshes")
            .get(gi)
            .ok_or_else(|| malformed(format!("mesh index {gi} out of range")))?
            .clone();
        let mut out = SceneMesh {
            name: m.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string),
            ..Default::default()
        };
        let prims = m.get("primitives").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for (pi, prim) in prims.iter().enumerate() {
            if let Some(p) = self.primitive(gi, pi, prim)? {
                out.primitives.push(p);
            }
        }
        let targets = out.primitives.first().map_or(0, |p| p.targets.len());
        if let Some(w) = m.get("weights") {
            let a = w.as_array().ok_or_else(|| malformed(format!("mesh {gi} weights is not an array")))?;
            let ws: Vec<f32> = weights_of(a, &format!("mesh {gi} weights"))?;
            if ws.len() == targets {
                out.weights = ws;
            } else {
                self.warnings.push(format!(
                    "mesh {gi} lists {} default weights for {targets} morph targets; they were ignored",
                    ws.len()
                ));
            }
        }
        if let Some(names) = m.pointer("/extras/targetNames").and_then(|v| v.as_array()) {
            let names: Vec<String> = names.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect();
            if names.len() == targets {
                out.target_names = names;
            }
        }
        self.scene.meshes.push(out);
        let i = self.scene.meshes.len() - 1;
        self.mesh_map.insert(gi, i);
        Ok(i)
    }

    fn primitive(&mut self, gi: usize, pi: usize, prim: &Value) -> Result<Option<Primitive>, MeshError> {
        let where_ = format!("mesh {gi} primitive {pi}");
        let mode = match prim.get("mode") {
            None => MODE_TRIANGLES,
            Some(m) => m
                .as_u64()
                .ok_or_else(|| malformed(format!("{where_} has a non-numeric mode")))?,
        };
        match mode {
            // A strip and a fan are a triangle list written down more
            // compactly, and the spec says exactly which triangles they stand
            // for — there is nothing here to guess.
            MODE_TRIANGLES | MODE_TRIANGLE_STRIP | MODE_TRIANGLE_FAN => {}
            // Points and lines are topologies of their own (`Topology`). A
            // loop and a strip are a line list written down more compactly,
            // as a strip and a fan are a triangle list.
            MODE_POINTS | MODE_LINES | MODE_LINE_LOOP | MODE_LINE_STRIP => {}
            other => {
                self.warnings.push(format!("{where_} has mode {other}, which glTF 2.0 does not define (0 to 6); it was skipped"));
                return Ok(None);
            }
        }
        let attrs = prim
            .get("attributes")
            .and_then(|v| v.as_object())
            .ok_or_else(|| malformed(format!("{where_} has no attributes")))?;
        // glTF 2.0 §3.7.2.1: "When positions are not specified, client
        // implementations SHOULD skip primitive's rendering".
        let Some(pos_index) = index_field(attrs.get("POSITION"), &format!("{where_} POSITION"))? else {
            self.warnings.push(format!("{where_} has no POSITION attribute; it was skipped"));
            return Ok(None);
        };
        // KHR_draco_mesh_compression: the stream is decoded first, and its
        // values stand for the accessors it maps (README :131-:132), whether
        // the extension is required or only used: a used one's plain data is
        // never read (Ruling B27).
        let mut draco = match prim.pointer("/extensions/KHR_draco_mesh_compression") {
            None => None,
            // Only modes 0 to 6 get here (the mode match skips the rest), so
            // what is not TRIANGLES or TRIANGLE_STRIP is a point or line mode
            // or the fan (Ruling I10-R6a).
            Some(_) if mode != MODE_TRIANGLES && mode != MODE_TRIANGLE_STRIP => {
                let name = match mode {
                    MODE_POINTS => "POINTS",
                    MODE_LINES => "LINES",
                    MODE_LINE_LOOP => "LINE_LOOP",
                    MODE_LINE_STRIP => "LINE_STRIP",
                    _ => "TRIANGLE_FAN",
                };
                return Err(malformed(format!(
                    "{where_} is a Draco-compressed {name}; KHR_draco_mesh_compression allows TRIANGLES and TRIANGLE_STRIP"
                )))
            }
            Some(ext) => Some(self.g.decode_draco(ext, attrs, &where_)?),
        };
        if let Some(d) = &draco {
            for (name, values) in &d.attributes {
                let index = index_field(attrs.get(name), &format!("{where_} {name}"))?;
                let declared = index.and_then(|i| self.g.array("accessors").get(i)).and_then(|a| a.get("count")).and_then(as_usize);
                if let (Some(i), Some(declared)) = (index, declared) {
                    if declared != values.count() {
                        self.warnings.push(format!(
                            "{where_} {name} accessor {i} declares {declared} elements, but its Draco stream decodes {}; the decoded ones were read",
                            values.count()
                        ));
                    }
                }
            }
            // The same for `indices`, whose data the stream's faces stand
            // for (Ruling B32; Task 4 minor 2).
            if let Some(i) = index_field(prim.get("indices"), &format!("{where_} indices"))? {
                let declared = self.g.array("accessors").get(i).and_then(|a| a.get("count")).and_then(as_usize);
                if let Some(declared) = declared.filter(|&declared| declared != d.indices.len()) {
                    self.warnings.push(format!(
                        "{where_} indices accessor {i} declares {declared} elements, but its Draco stream decodes {}; the decoded ones were read",
                        d.indices.len()
                    ));
                }
            }
        }
        let g = self.g;
        let total_vertices = self.total_vertices;
        let quantized = g.quantized;
        // KHR_mesh_quantization allows integer positions, normalised or not;
        // core glTF allows only FLOAT.
        let positions = chunk3(g.read_attribute(pos_index, "POSITION", draco.as_ref(), &where_, |pos_acc| {
            // Checked before the read, not after: a Vec sized past the
            // ceiling is one the allocator may never hand back. A Draco
            // primitive's count is what its stream decoded.
            if total_vertices.saturating_add(pos_acc.count) > MAX_MERGED_VERTICES {
                return Err(unsupported(format!(
                    "the merged mesh passes {MAX_MERGED_VERTICES} vertices at {where_}; \
                     the document describes more geometry than this converter will hold"
                )));
            }
            pos_acc.read_f32(&[3], "POSITION", |c, _| {
                c == COMP_FLOAT
                    || (quantized && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
            }, &g.budget)
        })?);
        let vcount = positions.len();
        self.total_vertices += vcount;

        let count_ok = |len: usize, what: &str| -> Result<(), MeshError> {
            if len != vcount {
                Err(malformed(format!("{where_} has {len} {what} for {vcount} positions")))
            } else {
                Ok(())
            }
        };

        let normals = match index_field(attrs.get("NORMAL"), &format!("{where_} NORMAL"))? {
            None => None,
            Some(i) => {
                let v = chunk3(g.read_attribute(i, "NORMAL", draco.as_ref(), &where_, |acc| {
                    acc.read_f32(&[3], "NORMAL", |c, n| c == COMP_FLOAT || (quantized && n && matches!(c, COMP_BYTE | COMP_SHORT)), &g.budget)
                })?);
                count_ok(v.len(), "normals")?;
                Some(v)
            }
        };
        let tangents = match index_field(attrs.get("TANGENT"), &format!("{where_} TANGENT"))? {
            None => None,
            Some(i) => {
                let v = chunk4(g.read_attribute(i, "TANGENT", draco.as_ref(), &where_, |acc| {
                    acc.read_f32(&[4], "TANGENT", |c, n| c == COMP_FLOAT || (quantized && n && matches!(c, COMP_BYTE | COMP_SHORT)), &g.budget)
                })?);
                count_ok(v.len(), "tangents")?;
                Some(v)
            }
        };
        let mut uvs = Vec::new();
        for set in 0..MAX_UV_SETS {
            let key = format!("TEXCOORD_{set}");
            let Some(i) = index_field(attrs.get(&key), &format!("{where_} {key}"))? else { break };
            let v = chunk2(g.read_attribute(i, &key, draco.as_ref(), &where_, |acc| {
                acc.read_f32(&[2], &key, |c, n| {
                    float_or_unorm(c, n)
                        || (quantized && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
                }, &g.budget)
            })?);
            count_ok(v.len(), "uvs")?;
            uvs.push(v);
        }
        let colors = match index_field(attrs.get("COLOR_0"), &format!("{where_} COLOR_0"))? {
            None => None,
            Some(i) => {
                let (n, raw) = g.read_attribute(i, "COLOR_0", draco.as_ref(), &where_, |acc| {
                    Ok((acc.ncomp, acc.read_f32(&[3, 4], "COLOR_0", float_or_unorm, &g.budget)?))
                })?;
                let v: Vec<[f32; 4]> = if n == 3 {
                    raw.chunks_exact(3).map(|c| [c[0], c[1], c[2], 1.0]).collect()
                } else {
                    chunk4(raw)
                };
                count_ok(v.len(), "colours")?;
                Some(v)
            }
        };
        let mut joint_sets: Vec<Vec<u32>> = Vec::new();
        let mut weight_sets: Vec<Vec<f32>> = Vec::new();
        for set in 0..MAX_INFLUENCE_SETS {
            let jk = format!("JOINTS_{set}");
            let wk = format!("WEIGHTS_{set}");
            let j = index_field(attrs.get(&jk), &format!("{where_} {jk}"))?;
            let w = index_field(attrs.get(&wk), &format!("{where_} {wk}"))?;
            match (j, w) {
                (None, None) => break,
                (Some(j), Some(w)) => {
                    let js = g.read_attribute(j, &jk, draco.as_ref(), &where_, |acc| {
                        acc.read_u32(4, &jk, &[COMP_UNSIGNED_BYTE, COMP_UNSIGNED_SHORT], &g.budget)
                    })?;
                    let ws = g.read_attribute(w, &wk, draco.as_ref(), &where_, |acc| acc.read_f32(&[4], &wk, float_or_unorm, &g.budget))?;
                    count_ok(js.len() / 4, "joint influences")?;
                    count_ok(ws.len() / 4, "weights")?;
                    joint_sets.push(js);
                    weight_sets.push(ws);
                }
                _ => {
                    return Err(malformed(format!(
                        "{where_} has {jk} without {wk} or the reverse; influences come in pairs"
                    )))
                }
            }
        }
        let skin = if joint_sets.is_empty() {
            None
        } else {
            let sets = joint_sets.len();
            let influences = sets * 4;
            let mut joints = Vec::with_capacity(vcount * influences);
            let mut weights = Vec::with_capacity(vcount * influences);
            for v in 0..vcount {
                for s in 0..sets {
                    for k in 0..4 {
                        let j = joint_sets[s][v * 4 + k];
                        joints.push(u16::try_from(j).map_err(|_| malformed(format!("{where_} joint index {j} exceeds 65535")))?);
                        weights.push(weight_sets[s][v * 4 + k]);
                    }
                }
            }
            Some(SkinWeights { influences, joints, weights })
        };

        // A Draco primitive's triangles are its stream's faces: a triangle
        // list whatever the mode, so a TRIANGLE_STRIP's is never expanded a
        // second time (Review Focus 4). The `indices` accessor's own data,
        // like every mapped accessor's, is ignored (README :132).
        let mode = if draco.is_some() { MODE_TRIANGLES } else { mode };
        // The vertex order the primitive's mode is written in; what it means as
        // triangles is decided just below.
        let sequence = match (draco.as_mut(), index_field(prim.get("indices"), &format!("{where_} indices"))?) {
            (Some(d), _) => {
                g.budget.charge(d.indices.len(), &format!("{where_} {DRACO_EXTENSION} indices"))?;
                std::mem::take(&mut d.indices)
            }
            (None, Some(i)) => g.accessor(i)?.read_indices(&g.budget)?,
            // A primitive with no `indices` walks its vertices in order — for a
            // triangle list that is 0,1,2 then 3,4,5.
            (None, None) => {
                if mode == MODE_TRIANGLES && !vcount.is_multiple_of(3) {
                    return Err(malformed(format!(
                        "{where_} is non-indexed with {vcount} vertices, not a multiple of 3"
                    )));
                }
                g.budget.charge(vcount, "synthesised indices")?;
                (0..vcount as u32).collect()
            }
        };
        // What the primitive draws, from the mode as it stands after the Draco
        // line above: a Draco stream is triangles, so it is never points or
        // lines.
        let topology = match mode {
            MODE_POINTS => super::scene::Topology::Points,
            MODE_LINES | MODE_LINE_LOOP | MODE_LINE_STRIP => super::scene::Topology::Lines,
            _ => super::scene::Topology::Triangles,
        };
        // A strip or a fan is three indices per vertex past the first two, a
        // line strip two per vertex past the first and a loop two per vertex,
        // so the ceiling is checked against what the expansion will produce
        // rather than after the Vec has been paid for.
        let expanded = match mode {
            MODE_TRIANGLES | MODE_POINTS | MODE_LINES => sequence.len(),
            MODE_LINE_STRIP => sequence.len().saturating_sub(1).saturating_mul(2),
            MODE_LINE_LOOP => sequence.len().saturating_mul(2),
            _ => sequence.len().saturating_sub(2).saturating_mul(3),
        };
        g.budget.charge_indices(expanded, &where_)?;
        if !matches!(mode, MODE_TRIANGLES | MODE_POINTS | MODE_LINES) {
            // The expansion is a new Vec, two or three times the sequence's length.
            let list = if topology == super::scene::Topology::Triangles { "triangle" } else { "line" };
            g.budget.charge(expanded, &format!("{where_} as a {list} list"))?;
        }
        let indices = match mode {
            MODE_TRIANGLE_STRIP => strip_to_triangles(&sequence, &where_)?,
            MODE_TRIANGLE_FAN => fan_to_triangles(&sequence, &where_)?,
            MODE_LINE_STRIP => line_strip_to_lines(&sequence, &where_)?,
            MODE_LINE_LOOP => line_loop_to_lines(&sequence, &where_)?,
            _ => sequence,
        };
        let per_element = topology.indices_per_element();
        if !indices.len().is_multiple_of(per_element) {
            return Err(malformed(format!(
                "{where_} has {} indices, not a multiple of {per_element}",
                indices.len()
            )));
        }
        if let Some(bad) = indices.iter().find(|i| **i as usize >= vcount) {
            return Err(malformed(format!(
                "{where_} index {bad} is out of range for {vcount} vertices"
            )));
        }

        let mut targets = Vec::new();
        if let Some(ts) = prim.get("targets") {
            let ts = ts.as_array().ok_or_else(|| malformed(format!("{where_} targets is not an array")))?;
            if ts.len() > super::scene::MAX_MORPH_TARGETS {
                return Err(unsupported(format!("{where_} has {} morph targets", ts.len())));
            }
            for (ti, t) in ts.iter().enumerate() {
                let tp = match index_field(t.get("POSITION"), &format!("{where_} target {ti} POSITION"))? {
                    Some(i) => {
                        let v = chunk3(self.g.accessor(i)?.read_f32(&[3], "morph POSITION", |c, _| {
                            c == COMP_FLOAT || (quantized && matches!(c, COMP_BYTE | COMP_SHORT))
                        }, &self.g.budget)?);
                        count_ok(v.len(), "morph position offsets")?;
                        v
                    }
                    // A target that moves only normals: zero position offsets.
                    None => {
                        self.g.budget.charge(vcount * 3, "morph target")?;
                        vec![[0.0; 3]; vcount]
                    }
                };
                let tn = match index_field(t.get("NORMAL"), &format!("{where_} target {ti} NORMAL"))? {
                    Some(i) => {
                        let v = chunk3(self.g.accessor(i)?.read_f32(&[3], "morph NORMAL", |c, n| {
                            c == COMP_FLOAT || (quantized && n && matches!(c, COMP_BYTE | COMP_SHORT))
                        }, &self.g.budget)?);
                        count_ok(v.len(), "morph normal offsets")?;
                        Some(v)
                    }
                    None => None,
                };
                targets.push(MorphTarget { positions: tp, normals: tn });
            }
        }
        // The scene keeps target normals all-or-nothing per primitive.
        if targets.iter().any(|t| t.normals.is_some()) {
            for t in targets.iter_mut() {
                if t.normals.is_none() {
                    t.normals = Some(vec![[0.0; 3]; vcount]);
                }
            }
        }

        let material = match index_field(prim.get("material"), "primitive material")? {
            None => None,
            Some(mi) => Some(self.material(mi)?),
        };

        Ok(Some(Primitive {
            positions,
            normals,
            tangents,
            uvs,
            colors,
            skin,
            indices,
            material,
            targets,
            topology,
        }))
    }

    fn material(&mut self, gi: usize) -> Result<usize, MeshError> {
        if let Some(&i) = self.material_map.get(&gi) {
            return Ok(i);
        }
        let m = self
            .g
            .array("materials")
            .get(gi)
            .ok_or_else(|| malformed(format!("material index {gi} out of range")))?
            .clone();
        let at = format!("material {gi}");
        let pbr = m.get("pbrMetallicRoughness");
        let bcf = pbr.and_then(|p| p.get("baseColorFactor"));
        if let Some(f) = bcf {
            if !f.as_array().is_some_and(|a| a.len() == 4) {
                return Err(malformed(format!("{at} baseColorFactor is not a 4-array")));
            }
        }
        let base = numbers_field::<4>(bcf, [1.0; 4], &format!("{at} baseColorFactor"))
            .map_err(|_| malformed(format!("{at} baseColorFactor is not finite")))?;
        let emissive = numbers_field::<3>(m.get("emissiveFactor"), [0.0; 3], &format!("{at} emissiveFactor"))?;
        let strength = number_field(
            m.pointer("/extensions/KHR_materials_emissive_strength/emissiveStrength"),
            1.0,
            &format!("{at} emissiveStrength"),
        )?;
        // The spec's spellings are upper case; a writer's lower-case one
        // means the same, and an unknown one leaves the default — a material
        // property is no reason to refuse the geometry.
        let alpha_mode = match m.get("alphaMode").and_then(|v| v.as_str()) {
            None => AlphaMode::Opaque,
            Some(s) if s.eq_ignore_ascii_case("OPAQUE") => AlphaMode::Opaque,
            Some(s) if s.eq_ignore_ascii_case("MASK") => AlphaMode::Mask,
            Some(s) if s.eq_ignore_ascii_case("BLEND") => AlphaMode::Blend,
            Some(other) => {
                self.warnings.push(format!("{at} alphaMode {other:?} is not a glTF mode; read as OPAQUE"));
                AlphaMode::Opaque
            }
        };
        if m.get("alphaMode").and_then(|v| v.as_str()).is_some_and(|s| !matches!(s, "OPAQUE" | "MASK" | "BLEND") && ["OPAQUE", "MASK", "BLEND"].iter().any(|k| s.eq_ignore_ascii_case(k))) {
            self.warnings.push(format!("{at} alphaMode is not upper case, as glTF spells it; read as {alpha_mode:?}"));
        }
        let mut out = Material {
            name: m.get("name").and_then(|v| v.as_str()).map(str::to_string),
            base_color: [base[0] as f32, base[1] as f32, base[2] as f32, base[3] as f32],
            metallic: number_field(pbr.and_then(|p| p.get("metallicFactor")), 1.0, &format!("{at} metallicFactor"))? as f32,
            roughness: number_field(pbr.and_then(|p| p.get("roughnessFactor")), 1.0, &format!("{at} roughnessFactor"))? as f32,
            emissive: {
                let e = [emissive[0] * strength, emissive[1] * strength, emissive[2] * strength];
                if !e.iter().all(|v| fits_f32(*v)) {
                    return Err(malformed(format!("{at} emissive times its strength is past a 32-bit float's range")));
                }
                [e[0] as f32, e[1] as f32, e[2] as f32]
            },
            alpha_mode,
            alpha_cutoff: number_field(m.get("alphaCutoff"), 0.5, &format!("{at} alphaCutoff"))? as f32,
            double_sided: m.get("doubleSided").and_then(|v| v.as_bool()).unwrap_or(false),
            unlit: m.pointer("/extensions/KHR_materials_unlit").is_some(),
            ..Default::default()
        };
        out.base_color_texture = self.texture_info(pbr.and_then(|p| p.get("baseColorTexture")), &at)?;
        out.metallic_roughness_texture = self.texture_info(pbr.and_then(|p| p.get("metallicRoughnessTexture")), &at)?;
        if let Some(n) = m.get("normalTexture") {
            out.normal_texture = self.texture_info(Some(n), &at)?;
            out.normal_scale = number_field(n.get("scale"), 1.0, &format!("{at} normalTexture scale"))? as f32;
        }
        if let Some(o) = m.get("occlusionTexture") {
            out.occlusion_texture = self.texture_info(Some(o), &at)?;
            out.occlusion_strength = number_field(o.get("strength"), 1.0, &format!("{at} occlusion strength"))? as f32;
        }
        out.emissive_texture = self.texture_info(m.get("emissiveTexture"), &at)?;
        // After the base maps, so their textures keep their order.
        self.material_layers(&m, &at, &mut out)?;
        self.scene.materials.push(out);
        let i = self.scene.materials.len() - 1;
        self.material_map.insert(gi, i);
        Ok(i)
    }

    /// glTF's PBR layer extensions on material `m`, into `out`: each one
    /// present is its layer ([`Reader::material_layer`]). One whose own JSON
    /// does not read (a value that is not an object, a factor, a textureInfo,
    /// a texture index past `textures`, an ior outside the schema) is dropped
    /// with a warning that names it, and the file reads on: a layer is
    /// optional, and a reader without it draws the base layer. Each is
    /// checked whole before any texture it names is read, so a dropped layer
    /// leaves no texture behind. A texture the document cannot give drops
    /// the layer it serves too, with a warning, and what the layer's other
    /// maps read is pruned at the end of the read; one a base map uses still
    /// refuses the file.
    fn material_layers(&mut self, m: &Value, at: &str, out: &mut Material) -> Result<(), MeshError> {
        let Some(exts) = m.get("extensions") else { return Ok(()) };
        let textures = self.g.array("textures").len();
        let mut layers = Material::default();
        for layer in MaterialLayer::ALL {
            let name = layer_extension(layer);
            let Some(e) = exts.get(name) else { continue };
            if let Err(err) = Self::material_layer(e, layer, textures, &mut layers) {
                layers.drop_layer(layer);
                let detail = match err {
                    MeshError::Malformed(d) => d,
                    other => other.to_string(),
                };
                self.warnings.push(format!("{at} {name}: {detail}; the layer was dropped"));
            }
        }
        // Only layer slots are bound here: `layers` has no base map. A
        // texture the document cannot give (a `source` that is no index, an
        // image or a sampler index out of range, …) drops the layer it
        // serves with a warning, as a malformed layer is dropped above
        // (Ruling A16c, revised): before item 4 no layer was read, and such a
        // file converted. A base map that uses it has refused the file
        // already, or will in a later material, as before.
        for layer in MaterialLayer::ALL {
            let mut broken = None;
            for (_, r) in layers.texture_slots_mut().filter(|(slot, _)| layer.texture_slots().contains(slot)) {
                let Some(unread) = r.take() else { continue };
                match self.resolve_texture(unread) {
                    Ok(resolved) => *r = resolved,
                    Err(MeshError::Malformed(detail)) => {
                        broken = Some(detail);
                        break;
                    }
                    Err(other) => return Err(other),
                }
            }
            if let Some(detail) = broken {
                layers.drop_layer(layer);
                self.warnings.push(format!("{at} {}: {detail}; the layer was dropped", layer_extension(layer)));
                self.unreferenced = true;
            }
        }
        out.clearcoat = layers.clearcoat;
        out.transmission = layers.transmission;
        out.volume = layers.volume;
        out.ior = layers.ior;
        out.specular = layers.specular;
        out.sheen = layers.sheen;
        out.iridescence = layers.iridescence;
        out.anisotropy = layers.anisotropy;
        out.dispersion = layers.dispersion;
        Ok(())
    }

    /// The `layer` extension object `e` into `m`, checked whole without
    /// reading a texture; a value that is no object is refused, never read as
    /// `{}`. A field the object leaves out is the extension's default, as its
    /// schema states it, so `{}` is the layer at its defaults. Numbers are
    /// read as the base factors are and kept as the file states them, save
    /// an ior the schema refuses, for which `Scene::validate` would refuse
    /// the whole file. Each texture is a checked textureInfo whose `texture`
    /// is still the document's index, below `textures`.
    fn material_layer(e: &Value, layer: MaterialLayer, textures: usize, m: &mut Material) -> Result<(), MeshError> {
        // Not `{}`: read as one, it would be a layer the file never had.
        if !e.is_object() {
            return Err(malformed(format!("the extension is {}, which is not an object", shown_json(e, 60))));
        }
        let num = |key: &str, default: f64| number_field(e.get(key), default, key).map(|v| v as f32);
        let rgb = |key: &str, default: [f64; 3]| numbers_field::<3>(e.get(key), default, key).map(|c| c.map(|v| v as f32));
        let tex = |key: &str| -> Result<Option<TextureRef>, MeshError> {
            let Some(info) = e.get(key) else { return Ok(None) };
            let r = Self::texture_info_fields(info, key)?;
            if r.texture >= textures {
                return Err(malformed(format!("{key} texture index {} is out of range", r.texture)));
            }
            Ok(Some(r))
        };
        match layer {
            MaterialLayer::Clearcoat => {
                m.clearcoat = Some(Clearcoat {
                    factor: num("clearcoatFactor", 0.0)?,
                    texture: tex("clearcoatTexture")?,
                    roughness: num("clearcoatRoughnessFactor", 0.0)?,
                    roughness_texture: tex("clearcoatRoughnessTexture")?,
                    normal_texture: tex("clearcoatNormalTexture")?,
                    // A normalTextureInfo: its `scale` is the coat normal's.
                    normal_scale: number_field(e.pointer("/clearcoatNormalTexture/scale"), 1.0, "clearcoatNormalTexture scale")? as f32,
                })
            }
            MaterialLayer::Transmission => {
                m.transmission = Some(Transmission { factor: num("transmissionFactor", 0.0)?, texture: tex("transmissionTexture")? })
            }
            MaterialLayer::Volume => {
                m.volume = Some(Volume {
                    thickness: num("thicknessFactor", 0.0)?,
                    thickness_texture: tex("thicknessTexture")?,
                    // "When undefined, the value is assumed to be infinite."
                    attenuation_distance: num("attenuationDistance", f64::INFINITY)?,
                    attenuation_color: rgb("attenuationColor", [1.0; 3])?,
                })
            }
            MaterialLayer::Ior => {
                let ior = num("ior", f64::from(super::scene::DEFAULT_IOR))?;
                if !super::scene::ior_in_schema(ior) {
                    return Err(malformed(format!("ior is {ior}, where the schema allows 0, or 1 and above")));
                }
                m.ior = Some(ior);
            }
            MaterialLayer::Specular => {
                m.specular = Some(Specular {
                    factor: num("specularFactor", 1.0)?,
                    texture: tex("specularTexture")?,
                    color: rgb("specularColorFactor", [1.0; 3])?,
                    color_texture: tex("specularColorTexture")?,
                })
            }
            MaterialLayer::Sheen => {
                m.sheen = Some(Sheen {
                    color: rgb("sheenColorFactor", [0.0; 3])?,
                    color_texture: tex("sheenColorTexture")?,
                    roughness: num("sheenRoughnessFactor", 0.0)?,
                    roughness_texture: tex("sheenRoughnessTexture")?,
                })
            }
            MaterialLayer::Iridescence => {
                m.iridescence = Some(Iridescence {
                    factor: num("iridescenceFactor", 0.0)?,
                    texture: tex("iridescenceTexture")?,
                    ior: num("iridescenceIor", 1.3)?,
                    thickness_min: num("iridescenceThicknessMinimum", 100.0)?,
                    thickness_max: num("iridescenceThicknessMaximum", 400.0)?,
                    thickness_texture: tex("iridescenceThicknessTexture")?,
                })
            }
            MaterialLayer::Anisotropy => {
                m.anisotropy = Some(Anisotropy {
                    strength: num("anisotropyStrength", 0.0)?,
                    rotation: num("anisotropyRotation", 0.0)?,
                    texture: tex("anisotropyTexture")?,
                })
            }
            MaterialLayer::Dispersion => m.dispersion = Some(num("dispersion", 0.0)?),
        }
        Ok(())
    }

    fn texture_info(&mut self, info: Option<&Value>, at: &str) -> Result<Option<TextureRef>, MeshError> {
        let Some(info) = info else { return Ok(None) };
        let r = Self::texture_info_fields(info, at)?;
        self.resolve_texture(r)
    }

    /// A textureInfo's own fields, checked, with `texture` still the
    /// document's index: what [`Reader::resolve_texture`] reads.
    fn texture_info_fields(info: &Value, at: &str) -> Result<TextureRef, MeshError> {
        let ti = index_field(info.get("index"), &format!("{at} texture index"))?
            .ok_or_else(|| malformed(format!("{at} textureInfo has no index")))?;
        let mut uv_set = index_field(info.get("texCoord"), &format!("{at} texCoord"))?.unwrap_or(0) as u32;
        let transform = match info.pointer("/extensions/KHR_texture_transform") {
            None => None,
            Some(t) => {
                let off = numbers_field::<2>(t.get("offset"), [0.0, 0.0], &format!("{at} texture offset"))?;
                let sc = numbers_field::<2>(t.get("scale"), [1.0, 1.0], &format!("{at} texture scale"))?;
                let rot = number_field(t.get("rotation"), 0.0, &format!("{at} texture rotation"))?;
                if let Some(tc) = index_field(t.get("texCoord"), &format!("{at} transform texCoord"))? {
                    uv_set = tc as u32;
                }
                let tr = UvTransform {
                    offset: [off[0] as f32, off[1] as f32],
                    rotation: rot as f32,
                    scale: [sc[0] as f32, sc[1] as f32],
                };
                (!tr.is_identity()).then_some(tr)
            }
        };
        Ok(TextureRef { texture: ti, uv_set, transform })
    }

    /// `r`, which names the document's texture, naming the scene's instead;
    /// `None` when that texture was dropped.
    fn resolve_texture(&mut self, r: TextureRef) -> Result<Option<TextureRef>, MeshError> {
        Ok(self.texture(r.texture)?.map(|texture| TextureRef { texture, ..r }))
    }

    fn texture(&mut self, gi: usize) -> Result<Option<usize>, MeshError> {
        if let Some(&i) = self.texture_map.get(&gi) {
            return Ok(i);
        }
        let t = self
            .g
            .array("textures")
            .get(gi)
            .ok_or_else(|| malformed(format!("texture index {gi} out of range")))?
            .clone();
        // A PNG/JPEG `source` is the portable fallback; the WebP/KTX2 source in
        // an extension is used only when there is no fallback at all.
        let source = index_field(t.get("source"), &format!("texture {gi} source"))?
            .or(index_field(t.pointer("/extensions/EXT_texture_webp/source"), &format!("texture {gi} webp source"))?)
            .or(index_field(t.pointer("/extensions/KHR_texture_basisu/source"), &format!("texture {gi} basisu source"))?);
        let Some(source) = source else {
            self.warnings.push(format!("texture {gi} has no image source; it was dropped"));
            self.texture_map.insert(gi, None);
            return Ok(None);
        };
        let Some(image) = self.image(source)? else {
            self.texture_map.insert(gi, None);
            return Ok(None);
        };
        let sampler = match index_field(t.get("sampler"), &format!("texture {gi} sampler"))? {
            None => Sampler::default(),
            Some(si) => self.sampler(si)?,
        };
        self.scene.textures.push(Texture {
            name: t.get("name").and_then(|v| v.as_str()).map(str::to_string),
            image,
            sampler,
        });
        let i = self.scene.textures.len() - 1;
        self.texture_map.insert(gi, Some(i));
        Ok(Some(i))
    }

    fn sampler(&mut self, si: usize) -> Result<Sampler, MeshError> {
        let s = self
            .g
            .array("samplers")
            .get(si)
            .ok_or_else(|| malformed(format!("sampler index {si} out of range")))?;
        // A value outside the spec's enum is read as the default, with a
        // warning: sampling is no reason to refuse the geometry.
        let mut quirks: Vec<String> = Vec::new();
        let mut wrap = |k: &str| -> Result<Wrap, MeshError> {
            Ok(match index_field(s.get(k), &format!("sampler {si} {k}"))? {
                None | Some(10497) => Wrap::Repeat,
                Some(33071) => Wrap::ClampToEdge,
                Some(33648) => Wrap::MirroredRepeat,
                Some(other) => {
                    quirks.push(format!("sampler {si} {k} is {other}, not a glTF wrap mode; read as REPEAT"));
                    Wrap::Repeat
                }
            })
        };
        let (wrap_s, wrap_t) = (wrap("wrapS")?, wrap("wrapT")?);
        let mag = match index_field(s.get("magFilter"), &format!("sampler {si} magFilter"))? {
            None => None,
            Some(9728) => Some(Filter::Nearest),
            Some(9729) => Some(Filter::Linear),
            Some(other) => {
                quirks.push(format!("sampler {si} magFilter is {other}, not NEAREST or LINEAR; left to the viewer"));
                None
            }
        };
        let (min, mipmaps, mipmap_filter) =
            match index_field(s.get("minFilter"), &format!("sampler {si} minFilter"))? {
                None => (None, true, Filter::Linear),
                Some(9728) => (Some(Filter::Nearest), false, Filter::Linear),
                Some(9729) => (Some(Filter::Linear), false, Filter::Linear),
                Some(9984) => (Some(Filter::Nearest), true, Filter::Nearest),
                Some(9985) => (Some(Filter::Linear), true, Filter::Nearest),
                Some(9986) => (Some(Filter::Nearest), true, Filter::Linear),
                Some(9987) => (Some(Filter::Linear), true, Filter::Linear),
                Some(other) => {
                    quirks.push(format!("sampler {si} minFilter is {other}, not a glTF filter; left to the viewer"));
                    (None, true, Filter::Linear)
                }
            };
        self.warnings.extend(quirks);
        Ok(Sampler {
            wrap_s,
            wrap_t,
            mag_filter: mag,
            min_filter: min,
            mipmaps,
            mipmap_filter,
        })
    }

    fn image(&mut self, gi: usize) -> Result<Option<usize>, MeshError> {
        if let Some(&i) = self.image_map.get(&gi) {
            return Ok(i);
        }
        let im = self
            .g
            .array("images")
            .get(gi)
            .ok_or_else(|| malformed(format!("image index {gi} out of range")))?
            .clone();
        let bytes: Option<Vec<u8>> = if let Some(v) = index_field(im.get("bufferView"), &format!("image {gi} bufferView"))? {
            let (data, _) = self.g.view_bytes(v)?;
            self.g.budget.charge_image(data.len())?;
            Some(data.to_vec())
        } else if let Some(uri) = im.get("uri").and_then(|u| u.as_str()) {
            if package::is_data_uri(uri) {
                let (_, b) = decode_data_uri(uri, &format!("image {gi}"))?;
                self.g.budget.charge_image(b.len())?;
                Some(b)
            } else {
                match package::normalize_relative(uri).and_then(|p| self.sidecars.get(&p)) {
                    Some(b) => {
                        self.g.budget.charge_image(b.len())?;
                        Some(b)
                    }
                    None => {
                        self.warnings.push(format!(
                            "image {gi} refers to {:?}, which was not supplied; its textures were dropped",
                            truncate(uri, 120)
                        ));
                        None
                    }
                }
            }
        } else {
            return Err(malformed(format!("image {gi} has neither a bufferView nor a uri")));
        };
        let out = match bytes {
            None => None,
            Some(data) => match ImageFormat::sniff(&data) {
                Some(format) => {
                    self.scene.images.push(Image {
                        name: im.get("name").and_then(|v| v.as_str()).map(str::to_string),
                        format,
                        data,
                    });
                    Some(self.scene.images.len() - 1)
                }
                None => {
                    // The label is not trusted (see `ImageFormat::sniff`), and
                    // bytes that are no known image cannot be passed on as one.
                    self.warnings.push(format!(
                        "image {gi} is not PNG, JPEG, WebP or KTX2; its textures were dropped"
                    ));
                    None
                }
            },
        };
        self.image_map.insert(gi, out);
        Ok(out)
    }

    fn skin(&mut self, gi: usize) -> Result<usize, MeshError> {
        if let Some(&i) = self.skin_map.get(&gi) {
            return Ok(i);
        }
        let s = self
            .g
            .array("skins")
            .get(gi)
            .ok_or_else(|| malformed(format!("skin index {gi} out of range")))?
            .clone();
        let joints_raw = s
            .get("joints")
            .and_then(|v| v.as_array())
            .ok_or_else(|| malformed(format!("skin {gi} has no joints")))?;
        if joints_raw.is_empty() || joints_raw.len() > super::scene::MAX_JOINTS {
            return Err(malformed(format!("skin {gi} has {} joints", joints_raw.len())));
        }
        let mut joints = Vec::with_capacity(joints_raw.len());
        for (k, j) in joints_raw.iter().enumerate() {
            let gj = as_usize(j).ok_or_else(|| malformed(format!("skin {gi} joint {k} is not an index")))?;
            let ir = self.node_map.get(&gj).copied().ok_or_else(|| {
                malformed(format!("skin {gi} joint {gj} is not a node of the scene being read"))
            })?;
            joints.push(ir);
        }
        let ibms = match index_field(s.get("inverseBindMatrices"), &format!("skin {gi} inverseBindMatrices"))? {
            None => vec![math::IDENTITY; joints.len()],
            Some(ai) => {
                let acc = self.g.accessor(ai)?;
                if acc.ncomp != 16 {
                    return Err(malformed(format!("skin {gi} inverseBindMatrices accessor is not MAT4")));
                }
                let raw = acc.read_f32(&[16], "inverseBindMatrices", float_only, &self.g.budget)?;
                if raw.len() / 16 < joints.len() {
                    return Err(malformed(format!(
                        "skin {gi} has {} inverse bind matrices for {} joints",
                        raw.len() / 16,
                        joints.len()
                    )));
                }
                raw.chunks_exact(16)
                    .take(joints.len())
                    .map(|c| {
                        let mut m = [0.0f64; 16];
                        for (o, v) in m.iter_mut().zip(c) {
                            *o = *v as f64;
                        }
                        m
                    })
                    .collect()
            }
        };
        let skeleton = match index_field(s.get("skeleton"), &format!("skin {gi} skeleton"))? {
            None => None,
            Some(n) => self.node_map.get(&n).copied(),
        };
        self.scene.skins.push(Skin {
            name: s.get("name").and_then(|v| v.as_str()).map(str::to_string),
            joints,
            inverse_bind_matrices: ibms,
            skeleton,
        });
        let i = self.scene.skins.len() - 1;
        self.skin_map.insert(gi, i);
        Ok(i)
    }

    fn animations(&mut self) -> Result<(), MeshError> {
        let anims = self.g.array("animations").to_vec();
        for (ai, a) in anims.iter().enumerate() {
            let samplers = a.get("samplers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let channels = a.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            let mut out = Animation {
                name: a.get("name").and_then(|v| v.as_str()).map(str::to_string),
                channels: Vec::new(),
            };
            for (ci, c) in channels.iter().enumerate() {
                let at = format!("animation {ai} channel {ci}");
                let target = c.get("target").ok_or_else(|| malformed(format!("{at} has no target")))?;
                let path = match target.get("path").and_then(|v| v.as_str()) {
                    Some("translation") => AnimPath::Translation,
                    Some("rotation") => AnimPath::Rotation,
                    Some("scale") => AnimPath::Scale,
                    Some("weights") => AnimPath::Weights,
                    Some(other) => {
                        // `pointer` (KHR_animation_pointer) and anything newer.
                        self.warnings.push(format!("{at} animates {other:?}, which is not modelled; skipped"));
                        continue;
                    }
                    None => return Err(malformed(format!("{at} has no target path"))),
                };
                let Some(gnode) = index_field(target.get("node"), &format!("{at} target node"))? else {
                    // Legal without the node when an extension supplies the
                    // target; nothing we model can be addressed that way.
                    continue;
                };
                let Some(&node) = self.node_map.get(&gnode) else {
                    // A node outside the scene being read: the channel animates
                    // something that is not in the output.
                    continue;
                };
                let si = index_field(c.get("sampler"), &format!("{at} sampler"))?
                    .ok_or_else(|| malformed(format!("{at} has no sampler")))?;
                let s = samplers.get(si).ok_or_else(|| malformed(format!("{at} sampler {si} is out of range")))?;
                let interpolation = match s.get("interpolation").and_then(|v| v.as_str()) {
                    None => Interpolation::Linear,
                    Some(i) if i.eq_ignore_ascii_case("LINEAR") => Interpolation::Linear,
                    Some(i) if i.eq_ignore_ascii_case("STEP") => Interpolation::Step,
                    Some(i) if i.eq_ignore_ascii_case("CUBICSPLINE") => Interpolation::CubicSpline,
                    Some(other) => {
                        self.warnings.push(format!("{at} interpolation {other:?} is not a glTF mode; read as LINEAR"));
                        Interpolation::Linear
                    }
                };
                if s.get("interpolation").and_then(|v| v.as_str()).is_some_and(|i| !matches!(i, "LINEAR" | "STEP" | "CUBICSPLINE") && ["LINEAR", "STEP", "CUBICSPLINE"].iter().any(|k| i.eq_ignore_ascii_case(k))) {
                    self.warnings.push(format!("{at} interpolation is not upper case, as glTF spells it"));
                }
                let input = index_field(s.get("input"), &format!("{at} input"))?
                    .ok_or_else(|| malformed(format!("{at} sampler has no input")))?;
                let output = index_field(s.get("output"), &format!("{at} output"))?
                    .ok_or_else(|| malformed(format!("{at} sampler has no output")))?;
                let times = self.g.accessor(input)?.read_f32(&[1], "animation input", float_only, &self.g.budget)?;
                let targets = self.scene.nodes[node].mesh.map_or(0, |m| self.scene.meshes[m].target_count());
                let out_acc = self.g.accessor(output)?;
                let values = match path {
                    AnimPath::Translation | AnimPath::Scale => out_acc.read_f32(&[3], "animation output", float_only, &self.g.budget)?,
                    AnimPath::Rotation => out_acc.read_f32(&[4], "animation output", |c, n| {
                        c == COMP_FLOAT || (n && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
                    }, &self.g.budget)?,
                    AnimPath::Weights => {
                        if targets == 0 {
                            self.warnings.push(format!("{at} animates weights of a node with no morph targets; skipped"));
                            continue;
                        }
                        out_acc.read_f32(&[1], "animation output", |c, n| {
                            c == COMP_FLOAT || (n && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
                        }, &self.g.budget)?
                    }
                };
                let mut ch = Channel { node, path, interpolation, times, values };
                let stride = ch.stride(targets);
                let per_key = if interpolation == Interpolation::CubicSpline { stride * 3 } else { stride };
                if ch.values.len() != ch.times.len() * per_key {
                    return Err(malformed(format!(
                        "{at} has {} output values for {} keys of {per_key}",
                        ch.values.len(),
                        ch.times.len()
                    )));
                }
                // The spec wants strictly increasing times; a key repeating the
                // time before it (some exporters write a hold so) is dropped,
                // keeping the first, and said so. Times that go backwards are
                // still refused by validation: their order means nothing.
                if ch.times.windows(2).any(|w| w[1] == w[0]) {
                    let (mut times, mut values) = (Vec::with_capacity(ch.times.len()), Vec::with_capacity(ch.values.len()));
                    for (k, t) in ch.times.iter().enumerate() {
                        if times.last() == Some(t) {
                            continue;
                        }
                        times.push(*t);
                        values.extend_from_slice(&ch.values[k * per_key..(k + 1) * per_key]);
                    }
                    self.warnings.push(format!("{at} repeats key times; the repeats were dropped"));
                    (ch.times, ch.values) = (times, values);
                }
                out.channels.push(ch);
            }
            if !out.channels.is_empty() {
                self.scene.animations.push(out);
            }
        }
        Ok(())
    }

    /// Walk the selected scene, building IR nodes in traversal order.
    fn nodes(&mut self) -> Result<(), MeshError> {
        let scenes = self.g.array("scenes").to_vec();
        let gnodes = self.g.array("nodes").to_vec();
        if scenes.is_empty() {
            // A glTF with meshes but no scene is legal (the spec calls it a
            // library) and a few pipelines emit one. Dropping every mesh would
            // report "empty" for a file that plainly has geometry, so take them
            // all at identity — there is no hierarchy to honour.
            for gm in 0..self.g.array("meshes").len() {
                let mesh = self.mesh(gm)?;
                self.scene.add_root(Node { mesh: Some(mesh), ..Default::default() });
            }
            return Ok(());
        }
        let scene_index = index_field(self.g.doc.get("scene"), "scene")?.unwrap_or(0);
        let gscene = scenes
            .get(scene_index)
            .ok_or_else(|| malformed(format!("scene index {scene_index} out of range")))?;
        self.scene.name = gscene.get("name").and_then(|v| v.as_str()).map(str::to_string);
        let roots: Vec<Value> = gscene.get("nodes").and_then(|v| v.as_array()).cloned().unwrap_or_default();

        // Pass 1: discover the reachable forest and allocate IR indices, so
        // skins (which name joints anywhere in it) can be resolved in pass 2.
        let mut visited = vec![false; gnodes.len()];
        let mut order: Vec<(usize, Option<usize>)> = Vec::new();
        // Explicit stack, not recursion: node depth is attacker-controlled and
        // a 100k-deep chain would blow the process stack rather than return Err.
        let mut stack: Vec<(usize, Option<usize>)> = Vec::new();
        for r in roots.iter().rev() {
            let i = r
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| malformed("scene node reference is not an index"))?;
            stack.push((i, None));
        }
        while let Some((gi, parent)) = stack.pop() {
            let node = gnodes.get(gi).ok_or_else(|| malformed(format!("node index {gi} out of range")))?;
            // glTF node graphs are strict forests. A node reached twice is
            // either a cycle (which would loop forever) or a shared subtree
            // (which would silently duplicate geometry) — both are the file
            // lying about its structure, so say so instead of coping.
            if std::mem::replace(&mut visited[gi], true) {
                return Err(malformed(format!(
                    "node {gi} is reachable more than once; the node graph is not a tree"
                )));
            }
            if order.len() >= super::scene::MAX_NODES {
                return Err(unsupported(format!("the scene has more than {} nodes", super::scene::MAX_NODES)));
            }
            let ir = order.len();
            order.push((gi, parent));
            self.node_map.insert(gi, ir);
            if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
                for c in children.iter().rev() {
                    let ci = c
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .ok_or_else(|| malformed(format!("node {gi} child is not an index")))?;
                    stack.push((ci, Some(ir)));
                }
            }
        }

        // Pass 2: build the nodes themselves.
        self.scene.nodes = vec![Node::default(); order.len()];
        for (ir, (gi, parent)) in order.iter().enumerate() {
            let node = &gnodes[*gi];
            let mut n = Node {
                name: node.get("name").and_then(|v| v.as_str()).map(str::to_string),
                transform: node_transform(node, *gi)?,
                ..Default::default()
            };
            // A `mesh` that does not read as an index is this node's geometry
            // going missing without a word, so it is the file's error and not a
            // node that happens to carry no mesh.
            if let Some(gm) = index_field(node.get("mesh"), &format!("node {gi} mesh"))? {
                let m = self.mesh(gm)?;
                // A mesh whose every primitive was points/lines contributes
                // nothing to draw; keep the node, drop the empty mesh link.
                if !self.scene.meshes[m].primitives.is_empty() {
                    n.mesh = Some(m);
                }
            }
            if let Some(w) = node.get("weights").and_then(|v| v.as_array()) {
                let ws = weights_of(w, &format!("node {gi} weights"))?;
                let targets = n.mesh.map_or(0, |m| self.scene.meshes[m].target_count());
                if ws.len() == targets && targets > 0 {
                    n.weights = Some(ws);
                }
            }
            n.camera = self.placed_camera(node, *gi);
            n.light = self.placed_light(node, *gi);
            self.scene.nodes[ir] = n;
            match parent {
                Some(p) => self.scene.nodes[*p].children.push(ir),
                None => self.scene.roots.push(ir),
            }
        }
        // Skins last: their joints must already have IR indices.
        for (ir, (gi, _)) in order.iter().enumerate() {
            if let Some(gs) = index_field(gnodes[*gi].get("skin"), &format!("node {gi} skin"))? {
                // A skin is only meaningful on a mesh that carries joint data;
                // on anything else glTF says it is ignored, and so it is here.
                let skinned = self.scene.nodes[ir]
                    .mesh
                    .is_some_and(|m| self.scene.meshes[m].primitives.iter().any(|p| p.skin.is_some()));
                if skinned {
                    let s = self.skin(gs)?;
                    self.scene.nodes[ir].skin = Some(s);
                }
            }
        }
        Ok(())
    }

    /// A node's reference to a light or camera (`field`, as the file spells
    /// it) as an index below `count`, the file's number of them: `None`, with
    /// a warning, for anything else. The node is kept either way.
    fn placed_index(&mut self, v: &Value, field: &str, kind: &str, gi: usize, count: usize) -> Option<usize> {
        match as_usize(v) {
            Some(i) if i < count => Some(i),
            Some(i) => {
                self.warnings.push(format!("node {gi} places {kind} {i}, past the file's {count} {kind}(s); the node is kept without it"));
                None
            }
            None => {
                self.warnings.push(format!("node {gi} {field} is {}, which is not an index; the node is kept without it", shown_json(v, 60)));
                None
            }
        }
    }

    /// The scene's index of the camera node `gi` places, read when a node
    /// first places it: `None` for a node that places none, and, with a
    /// warning that names it, for a camera that does not read or breaks a
    /// bound ([`Camera::check`]) and a reference that names no camera of the
    /// file. The node is kept either way: a reader never refuses a file over
    /// a light or camera. A node places one at most, so the node budget
    /// (`MAX_NODES`, charged in pass 1 before any node is built) bounds how
    /// many are built.
    fn placed_camera(&mut self, node: &Value, gi: usize) -> Option<usize> {
        let v = node.get("camera")?;
        let g = self.g;
        let all = g.array("cameras");
        let ci = self.placed_index(v, "camera", "camera", gi, all.len())?;
        if let Some(&kept) = self.camera_map.get(&ci) {
            return kept;
        }
        let kept = match camera_of(&all[ci]).and_then(|c| c.check().map(|()| c)) {
            Ok(c) => {
                self.scene.cameras.push(c);
                Some(self.scene.cameras.len() - 1)
            }
            Err(why) => {
                self.warnings.push(format!("{} is dropped: {why}", named("camera", ci, &all[ci])));
                None
            }
        };
        self.camera_map.insert(ci, kept);
        kept
    }

    /// Puts the scene's cameras in the file's order, after
    /// [`Reader::placed_camera`] read them in the order the nodes place
    /// them, and renumbers the nodes' cameras to match. GLTFLoader lists
    /// `gltf.cameras` in array order (`GLTFLoader.js` L2657, L2667 and
    /// `getDependencies`, L2975-L2996, r183), and apps (and the harness)
    /// take the first as the file's camera, so a conversion keeps which one
    /// that is.
    fn cameras_in_file_order(&mut self) {
        let mut order: Vec<(usize, usize)> = self.camera_map.iter().filter_map(|(&file, kept)| kept.map(|s| (file, s))).collect();
        order.sort_unstable();
        let mut renumbered: Vec<usize> = (0..self.scene.cameras.len()).collect();
        for (new, &(_, old)) in order.iter().enumerate() {
            if let Some(slot) = renumbered.get_mut(old) {
                *slot = new;
            }
        }
        let mut read: Vec<Option<Camera>> = std::mem::take(&mut self.scene.cameras).into_iter().map(Some).collect();
        self.scene.cameras = order.iter().filter_map(|&(_, old)| read.get_mut(old).and_then(Option::take)).collect();
        for c in self.scene.nodes.iter_mut().filter_map(|n| n.camera.as_mut()) {
            *c = renumbered.get(*c).copied().unwrap_or(*c);
        }
    }

    /// The scene's index of the `KHR_lights_punctual` light node `gi`
    /// places, read as [`Reader::placed_camera`] reads a camera, save that
    /// each of these is warned of and the light kept: a spot wider than a
    /// hemisphere is read as a point light ([`spot_past_a_hemisphere`],
    /// Ruling R32), a colour past 1 moves its peak into the intensity
    /// ([`colour_past_one`], Ruling R8), a hard-edged spot is softened
    /// ([`Light::soften_hard_edge`]), and a directional light's `range`,
    /// which glTF ignores, is ignored.
    fn placed_light(&mut self, node: &Value, gi: usize) -> Option<usize> {
        let ext = node.get("extensions")?.get("KHR_lights_punctual")?;
        let Some(v) = ext.get("light") else {
            self.warnings.push(format!("node {gi} KHR_lights_punctual is {}, which names no light; the node is kept without it", shown_json(ext, 60)));
            return None;
        };
        let g = self.g;
        let all = document_lights(g);
        let li = self.placed_index(v, "KHR_lights_punctual light", "light", gi, all.len())?;
        if let Some(&kept) = self.light_map.get(&li) {
            return kept;
        }
        let name = named("light", li, &all[li]);
        let read = light_of(&all[li]).and_then(|mut l| {
            let notes: Vec<String> = [spot_past_a_hemisphere(&mut l), colour_past_one(&mut l), l.soften_hard_edge()].into_iter().flatten().collect();
            l.check().map(|()| (l, notes))
        });
        let kept = match read {
            Ok((l, notes)) => {
                for how in notes {
                    self.warnings.push(format!("{name} {how}"));
                }
                if let (LightKind::Directional, Some(range)) = (l.kind, all[li].get("range")) {
                    self.warnings.push(format!("{name} is directional, so its range {} is ignored", shown_json(range, 60)));
                }
                self.scene.lights.push(l);
                Some(self.scene.lights.len() - 1)
            }
            Err(why) => {
                self.warnings.push(format!("{name} is dropped: {why}"));
                None
            }
        };
        self.light_map.insert(li, kept);
        kept
    }
}

/// Ruling R32: a spot wider than a hemisphere, its outer cone past π/2 (the
/// bound `Light::check` holds it to, as an f32), lights nearly every way, so
/// it is read as a point light and its cones go. What it did, worded to
/// follow `light "lamp" `, or `None` for any other light.
fn spot_past_a_hemisphere(l: &mut Light) -> Option<String> {
    let LightKind::Spot { outer_cone, .. } = l.kind else { return None };
    if !(outer_cone.is_finite() && outer_cone > std::f32::consts::FRAC_PI_2) {
        return None;
    }
    l.kind = LightKind::Point;
    Some(format!("has an outer cone of {outer_cone} rad, wider than a hemisphere (π/2), so it was read as a point light"))
}

/// Ruling R8: a colour component past 1, beyond KHR_lights_punctual's
/// maximum, holds part of the light's brightness, so the peak moves into
/// the intensity and the colour ends at 1, the light as bright as before.
/// What it did, worded to follow `light "lamp" `, or `None` for a colour
/// within 1, or one that is not finite (`Light::check` refuses that).
fn colour_past_one(l: &mut Light) -> Option<String> {
    let was = l.color;
    let peak = was.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !(peak > 1.0 && peak.is_finite()) {
        return None;
    }
    l.color = was.map(|c| c / peak);
    l.intensity *= peak;
    Some(format!(
        "has a colour of ({}, {}, {}), past glTF's 0 to 1, so its peak {peak} was moved into its intensity, now {}",
        was[0], was[1], was[2], l.intensity
    ))
}

/// The document's `KHR_lights_punctual` lights, none when it has no array
/// of them.
fn document_lights<'v>(g: &'v Gltf<'_>) -> &'v [Value] {
    g.doc.pointer("/extensions/KHR_lights_punctual/lights").and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// A light or camera as a warning names it: by its name, quoted and cut to
/// 60 characters (Ruling A45), or by its index in the file when it has none.
fn named(kind: &str, index: usize, v: &Value) -> String {
    match v.get("name") {
        Some(n) if n.as_str().is_some_and(|s| !s.is_empty()) => format!("{kind} {}", shown_json(n, 60)),
        _ => format!("{kind} {index}"),
    }
}

/// A number of a light or camera as an f32, `None` when absent. One past
/// f32's range comes out infinite, for `Light::check` or `Camera::check` to
/// name: JSON holds no NaN or infinity, so that is how a non-finite value
/// arrives.
fn f32_of(o: &Value, key: &str) -> Result<Option<f32>, String> {
    match o.get(key) {
        None => Ok(None),
        Some(v) => v.as_f64().map(|f| Some(f as f32)).ok_or_else(|| format!("its {key} is {}, which is not a number", shown_json(v, 60))),
    }
}

/// The object `key` of a light or camera, `None` when absent.
fn object_of<'v>(o: &'v Value, key: &str) -> Result<Option<&'v Value>, String> {
    match o.get(key) {
        None => Ok(None),
        Some(v) if v.is_object() => Ok(Some(v)),
        Some(v) => Err(format!("its {key} is {}, which is not an object", shown_json(v, 60))),
    }
}

/// One entry of `KHR_lights_punctual.lights` as the scene's light, each
/// field absent taking the schema's default (`color` white, `intensity` 1, a
/// spot's cones 0 and π/4), or why it does not read; its bounds are
/// `Light::check`'s to judge. `range` is read for a point or spot light
/// only, the lights glTF applies it to.
fn light_of(v: &Value) -> Result<Light, String> {
    if !v.is_object() {
        return Err(format!("it is {}, which is not an object", shown_json(v, 60)));
    }
    let t = v.get("type").ok_or("it has no type")?;
    let kind = match t.as_str() {
        Some("directional") => LightKind::Directional,
        Some("point") => LightKind::Point,
        Some("spot") => {
            let spot = object_of(v, "spot")?;
            let cone = |key: &str, default: f32| spot.map_or(Ok(None), |s| f32_of(s, key)).map(|c| c.unwrap_or(default));
            LightKind::Spot { inner_cone: cone("innerConeAngle", 0.0)?, outer_cone: cone("outerConeAngle", std::f32::consts::FRAC_PI_4)? }
        }
        _ => return Err(format!("its type {} is not directional, point or spot", shown_json(t, 60))),
    };
    let color = match v.get("color") {
        None => [1.0; 3],
        Some(c) => c
            .as_array()
            .filter(|a| a.len() == 3)
            .and_then(|a| Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32]))
            .ok_or_else(|| format!("its color is {}, which is not 3 numbers", shown_json(c, 60)))?,
    };
    Ok(Light {
        name: v.get("name").and_then(Value::as_str).map(str::to_string),
        kind,
        color,
        intensity: f32_of(v, "intensity")?.unwrap_or(1.0),
        range: if kind == LightKind::Directional { None } else { f32_of(v, "range")? },
    })
}

/// One entry of `cameras` as the scene's camera, or why it does not read:
/// a perspective one needs its `yfov` and `znear`, and an orthographic one
/// all four of its values, as glTF's schemas require; its bounds are
/// `Camera::check`'s to judge.
fn camera_of(v: &Value) -> Result<Camera, String> {
    if !v.is_object() {
        return Err(format!("it is {}, which is not an object", shown_json(v, 60)));
    }
    let t = v.get("type").ok_or("it has no type")?;
    let kind = match t.as_str() {
        Some(k @ ("perspective" | "orthographic")) => k,
        _ => return Err(format!("its type {} is not perspective or orthographic", shown_json(t, 60))),
    };
    let p = object_of(v, kind)?.ok_or_else(|| format!("it has no {kind}"))?;
    let need = |key: &str| f32_of(p, key)?.ok_or_else(|| format!("its {kind} has no {key}"));
    let projection = if kind == "perspective" {
        Projection::Perspective { yfov: need("yfov")?, aspect: f32_of(p, "aspectRatio")?, znear: need("znear")?, zfar: f32_of(p, "zfar")? }
    } else {
        Projection::Orthographic { xmag: need("xmag")?, ymag: need("ymag")?, znear: need("znear")?, zfar: need("zfar")? }
    };
    Ok(Camera { name: v.get("name").and_then(Value::as_str).map(str::to_string), projection })
}

/// Warnings for the entries of `all`, the file's lights or cameras, that no
/// node of the scene placed (`placed` holds those one did): each is dropped.
/// They are named one by one while the node budget lasts (`MAX_NODES`, as
/// many as a scene can place), charged before each warning is made; one more
/// warning counts any past it, so a file of millions cannot make millions of
/// warnings.
fn unplaced(kind: &str, all: &[Value], placed: &HashMap<usize, Option<usize>>, warnings: &mut Vec<String>) {
    let (mut named_so_far, mut more) = (0usize, 0usize);
    for (i, v) in all.iter().enumerate().filter(|(i, _)| !placed.contains_key(i)) {
        if named_so_far < super::scene::MAX_NODES {
            warnings.push(format!("{} is placed by no node of the scene, so it is dropped", named(kind, i, v)));
            named_so_far += 1;
        } else {
            more += 1;
        }
    }
    if more > 0 {
        warnings.push(format!("{more} more {kind}(s) are placed by no node of the scene, so they are dropped"));
    }
}

/// What a read produced besides the scene: things in the file that were
/// recognised and deliberately not carried, in words.
pub struct ReadOutcome {
    pub scene: Scene,
    pub warnings: Vec<String>,
}

fn read_document(doc: Value, bin: Option<&[u8]>, sidecars: &dyn Sidecars) -> Result<ReadOutcome, MeshError> {
    check_required_extensions(&doc)?;
    let uncarried = uncarried_extensions(&doc);
    let g = Gltf::new(doc, bin, sidecars);
    let mut r = Reader {
        g: &g,
        scene: Scene::default(),
        warnings: uncarried,
        mesh_map: HashMap::new(),
        material_map: HashMap::new(),
        texture_map: HashMap::new(),
        image_map: HashMap::new(),
        skin_map: HashMap::new(),
        node_map: HashMap::new(),
        camera_map: HashMap::new(),
        light_map: HashMap::new(),
        sidecars,
        total_vertices: 0,
        unreferenced: false,
    };
    r.nodes()?;
    r.cameras_in_file_order();
    unplaced("light", document_lights(&g), &r.light_map, &mut r.warnings);
    unplaced("camera", g.array("cameras"), &r.camera_map, &mut r.warnings);
    r.animations()?;
    // Skinned primitives whose node never got a skin (the file named none, or
    // named one we could not attach) render as rigid geometry; the influences
    // would otherwise reference a skeleton that is not there.
    let skinned_meshes: Vec<usize> = r.scene.nodes.iter().filter(|n| n.skin.is_some()).filter_map(|n| n.mesh).collect();
    for (mi, mesh) in r.scene.meshes.iter_mut().enumerate() {
        if !skinned_meshes.contains(&mi) {
            for p in mesh.primitives.iter_mut() {
                p.skin = None;
            }
        }
    }
    let Reader { mut scene, mut warnings, unreferenced, .. } = r;
    warnings.extend(g.notes.take());
    if unreferenced {
        super::pose::prune_unused_textures(&mut scene);
    }
    scene.validate()?;
    Ok(ReadOutcome { scene, warnings })
}

/// Read a glTF binary into a scene. External buffers and images are looked up
/// in `sidecars`.
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<ReadOutcome, MeshError> {
    let (json, bin) = split_chunks(bytes)?;
    let doc = parse_json(json)?;
    read_document(doc, bin, sidecars)
}

/// Read a glTF JSON document (`.gltf`) into a scene.
pub fn read_gltf_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<ReadOutcome, MeshError> {
    if bytes.starts_with(MAGIC) {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Gltf,
            detail: "starts with the GLB magic `glTF`; this is the binary container".into(),
        });
    }
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace() && *b != 0xEF && *b != 0xBB && *b != 0xBF);
    if start.and_then(|s| bytes.get(s)) != Some(&b'{') {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Gltf,
            detail: "a .gltf file is a JSON object and these bytes do not start with '{'".into(),
        });
    }
    let doc = parse_json(bytes).map_err(|e| match e {
        MeshError::Malformed(d) => MeshError::NotThisFormat { expected: MeshFormat::Gltf, detail: d },
        other => other,
    })?;
    if doc.pointer("/asset/version").and_then(|v| v.as_str()).is_none() {
        return Err(MeshError::NotThisFormat {
            expected: MeshFormat::Gltf,
            detail: "JSON without an asset.version is not a glTF document".into(),
        });
    }
    read_document(doc, None, sidecars)
}

/// Read a glTF 2.0 binary into the flat mesh: every primitive in world space,
/// merged. See `pose::flatten` for the exact semantics.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let outcome = read_scene(bytes, &NoSidecars)?;
    let mesh = super::pose::flatten(&outcome.scene)?;
    if mesh.indices.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

// ─── writer ─────────────────────────────────────────────────────────────────

fn pad_to_4(buf: &mut Vec<u8>, filler: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(filler);
    }
}

/// Accumulates the binary buffer, its views and accessors.
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        pad_to_4(&mut self.bin, 0);
        let start = self.bin.len();
        self.bin.extend_from_slice(bytes);
        let mut v = serde_json::json!({ "buffer": 0, "byteOffset": start, "byteLength": bytes.len() });
        if let Some(t) = target {
            v["target"] = Value::from(t);
        }
        self.views.push(v);
        self.views.len() - 1
    }

    fn floats(&mut self, data: &[f32], ncomp: usize, target: Option<u32>, min_max: bool) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        let mut a = serde_json::json!({
            "bufferView": view, "componentType": COMP_FLOAT,
            "count": data.len() / ncomp, "type": type_label(ncomp)
        });
        if min_max {
            let mut lo = vec![f32::INFINITY; ncomp];
            let mut hi = vec![f32::NEG_INFINITY; ncomp];
            for e in data.chunks_exact(ncomp) {
                for c in 0..ncomp {
                    lo[c] = lo[c].min(e[c]);
                    hi[c] = hi[c].max(e[c]);
                }
            }
            a["min"] = serde_json::json!(lo);
            a["max"] = serde_json::json!(hi);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn u16s(&mut self, data: &[u16], ncomp: usize, target: Option<u32>) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, target);
        self.accessors.push(serde_json::json!({
            "bufferView": view, "componentType": COMP_UNSIGNED_SHORT,
            "count": data.len() / ncomp, "type": type_label(ncomp)
        }));
        self.accessors.len() - 1
    }

    /// Indices are always UNSIGNED_INT. Narrowing to UNSIGNED_SHORT when the
    /// mesh has under 65536 vertices would save at most two bytes per index — a
    /// few percent of a file whose vertex attributes are 20+ bytes each — in
    /// exchange for a second code path and a size-dependent branch that only the
    /// small meshes in the test suite would ever take.
    fn indices(&mut self, data: &[u32]) -> usize {
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let view = self.view(&bytes, Some(TARGET_ELEMENT_ARRAY_BUFFER));
        self.accessors.push(serde_json::json!({
            "bufferView": view, "componentType": COMP_UNSIGNED_INT,
            "count": data.len(), "type": "SCALAR"
        }));
        self.accessors.len() - 1
    }
}

/// Where the writer puts images.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ImagePlacement {
    /// In the binary buffer (GLB, and `.gltf` with an embedded buffer).
    BufferView,
    /// As separate files beside a `.gltf`.
    Files,
}

struct Written {
    doc: Value,
    bin: Vec<u8>,
    image_files: Vec<PackageFile>,
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// Weights normalised so they sum to one in f32 — the validator checks the
/// sum, and three.js renormalises anyway, so the file should already agree.
fn normalized_weights(w: &[f32]) -> Vec<f32> {
    let sum: f64 = w.iter().map(|x| x.max(0.0) as f64).sum();
    if sum <= 0.0 {
        let mut out = vec![0.0f32; w.len()];
        if let Some(first) = out.first_mut() {
            *first = 1.0;
        }
        return out;
    }
    let mut out: Vec<f32> = w.iter().map(|x| (x.max(0.0) as f64 / sum) as f32).collect();
    // Put the f32 rounding residue on the largest weight, where it is
    // relatively smallest.
    let total: f32 = out.iter().sum();
    if let Some((imax, _)) = out.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)) {
        out[imax] += 1.0 - total;
    }
    out
}

fn texture_info_json(r: &TextureRef, tex_map: &[Option<usize>], extensions_used: &mut Vec<&'static str>) -> Option<Value> {
    let index = (*tex_map.get(r.texture)?)?;
    let mut v = serde_json::json!({ "index": index });
    if r.uv_set != 0 {
        v["texCoord"] = Value::from(r.uv_set);
    }
    if let Some(t) = &r.transform {
        if !extensions_used.contains(&"KHR_texture_transform") {
            extensions_used.push("KHR_texture_transform");
        }
        v["extensions"] = serde_json::json!({
            "KHR_texture_transform": { "offset": t.offset, "rotation": t.rotation, "scale": t.scale }
        });
    }
    Some(v)
}

/// One layer extension's object, field by field: a field at the extension's
/// default is left out, as glTF writers do.
struct LayerJson<'a> {
    fields: serde_json::Map<String, Value>,
    tex_map: &'a [Option<usize>],
    extensions_used: &'a mut Vec<&'static str>,
}

impl LayerJson<'_> {
    fn number(&mut self, key: &str, v: f32, default: f32) {
        if v != default {
            self.fields.insert(key.into(), Value::from(v));
        }
    }

    fn color(&mut self, key: &str, v: [f32; 3], default: [f32; 3]) {
        if v != default {
            self.fields.insert(key.into(), serde_json::json!(v));
        }
    }

    /// A textureInfo, written as the base maps' are; the object written.
    fn texture(&mut self, key: &str, r: &Option<TextureRef>) -> Option<&mut Value> {
        let t = texture_info_json(r.as_ref()?, self.tex_map, self.extensions_used)?;
        self.fields.insert(key.into(), t);
        self.fields.get_mut(key)
    }
}

/// Material `m`'s PBR layers as glTF's extensions, into `exts`: an object
/// for each layer it has, and none for a layer it lacks. Fields at their
/// defaults are left out ([`LayerJson`]), so a layer at its defaults is `{}`
/// and an infinite attenuation distance is no field. Each goes in
/// `extensionsUsed`, never `extensionsRequired`: a reader without it draws
/// the base layer, which is the extension's intent.
fn write_layer_extensions(m: &Material, tex_map: &[Option<usize>], extensions_used: &mut Vec<&'static str>, exts: &mut serde_json::Map<String, Value>) {
    for layer in MaterialLayer::ALL {
        let mut j = LayerJson { fields: serde_json::Map::new(), tex_map, extensions_used: &mut *extensions_used };
        match layer {
            MaterialLayer::Clearcoat => {
                let Some(c) = &m.clearcoat else { continue };
                j.number("clearcoatFactor", c.factor, 0.0);
                j.texture("clearcoatTexture", &c.texture);
                j.number("clearcoatRoughnessFactor", c.roughness, 0.0);
                j.texture("clearcoatRoughnessTexture", &c.roughness_texture);
                // A normalTextureInfo: its `scale` is the coat normal's.
                if let Some(t) = j.texture("clearcoatNormalTexture", &c.normal_texture) {
                    if c.normal_scale != 1.0 {
                        t["scale"] = Value::from(c.normal_scale);
                    }
                }
            }
            MaterialLayer::Transmission => {
                let Some(t) = &m.transmission else { continue };
                j.number("transmissionFactor", t.factor, 0.0);
                j.texture("transmissionTexture", &t.texture);
            }
            MaterialLayer::Volume => {
                let Some(v) = &m.volume else { continue };
                j.number("thicknessFactor", v.thickness, 0.0);
                j.texture("thicknessTexture", &v.thickness_texture);
                j.number("attenuationDistance", v.attenuation_distance, f32::INFINITY);
                j.color("attenuationColor", v.attenuation_color, [1.0; 3]);
            }
            MaterialLayer::Ior => {
                let Some(ior) = m.ior else { continue };
                j.number("ior", ior, 1.5);
            }
            MaterialLayer::Specular => {
                let Some(s) = &m.specular else { continue };
                j.number("specularFactor", s.factor, 1.0);
                j.texture("specularTexture", &s.texture);
                j.color("specularColorFactor", s.color, [1.0; 3]);
                j.texture("specularColorTexture", &s.color_texture);
            }
            MaterialLayer::Sheen => {
                let Some(s) = &m.sheen else { continue };
                j.color("sheenColorFactor", s.color, [0.0; 3]);
                j.texture("sheenColorTexture", &s.color_texture);
                j.number("sheenRoughnessFactor", s.roughness, 0.0);
                j.texture("sheenRoughnessTexture", &s.roughness_texture);
            }
            MaterialLayer::Iridescence => {
                let Some(i) = &m.iridescence else { continue };
                j.number("iridescenceFactor", i.factor, 0.0);
                j.texture("iridescenceTexture", &i.texture);
                j.number("iridescenceIor", i.ior, 1.3);
                j.number("iridescenceThicknessMinimum", i.thickness_min, 100.0);
                j.number("iridescenceThicknessMaximum", i.thickness_max, 400.0);
                j.texture("iridescenceThicknessTexture", &i.thickness_texture);
            }
            MaterialLayer::Anisotropy => {
                let Some(a) = &m.anisotropy else { continue };
                j.number("anisotropyStrength", a.strength, 0.0);
                j.number("anisotropyRotation", a.rotation, 0.0);
                j.texture("anisotropyTexture", &a.texture);
            }
            MaterialLayer::Dispersion => {
                let Some(d) = m.dispersion else { continue };
                j.number("dispersion", d, 0.0);
            }
        }
        let name = layer_extension(layer);
        exts.insert(name.into(), Value::Object(j.fields));
        if !extensions_used.contains(&name) {
            extensions_used.push(name);
        }
    }
}

/// A scene light as a `KHR_lights_punctual` light. Its colour and intensity
/// are written even at their defaults, as the PBR factors are, and `range`
/// only for a point or spot light, the lights glTF applies it to. An outer
/// cone of f32 π/2 holds `Light::check`, but as that f32 it is a hair past
/// the schema's maximum, the f64 π/2, so it is clamped there; it reads back
/// as the same f32.
fn light_json(l: &Light) -> Value {
    let mut o = serde_json::Map::new();
    if let Some(n) = &l.name {
        o.insert("name".into(), Value::from(n.clone()));
    }
    let kind = match l.kind {
        LightKind::Directional => "directional",
        LightKind::Point => "point",
        LightKind::Spot { inner_cone, outer_cone } => {
            let outer = f64::from(outer_cone).min(std::f64::consts::FRAC_PI_2);
            o.insert("spot".into(), serde_json::json!({ "innerConeAngle": inner_cone, "outerConeAngle": outer }));
            "spot"
        }
    };
    o.insert("type".into(), Value::from(kind));
    o.insert("color".into(), serde_json::json!(l.color));
    o.insert("intensity".into(), Value::from(l.intensity));
    if let (Some(range), false) = (l.range, l.kind == LightKind::Directional) {
        o.insert("range".into(), Value::from(range));
    }
    Value::Object(o)
}

/// A scene camera as a glTF one, with a perspective camera's `aspectRatio`
/// and `zfar` only when it has them.
fn camera_json(c: &Camera) -> Value {
    let mut o = serde_json::Map::new();
    if let Some(n) = &c.name {
        o.insert("name".into(), Value::from(n.clone()));
    }
    match c.projection {
        Projection::Perspective { yfov, aspect, znear, zfar } => {
            let mut p = serde_json::Map::new();
            p.insert("yfov".into(), Value::from(yfov));
            if let Some(a) = aspect {
                p.insert("aspectRatio".into(), Value::from(a));
            }
            p.insert("znear".into(), Value::from(znear));
            if let Some(f) = zfar {
                p.insert("zfar".into(), Value::from(f));
            }
            o.insert("type".into(), Value::from("perspective"));
            o.insert("perspective".into(), Value::Object(p));
        }
        Projection::Orthographic { xmag, ymag, znear, zfar } => {
            o.insert("type".into(), Value::from("orthographic"));
            o.insert("orthographic".into(), serde_json::json!({ "xmag": xmag, "ymag": ymag, "znear": znear, "zfar": zfar }));
        }
    }
    Value::Object(o)
}

/// Move every skinned mesh to an untransformed root node.
///
/// glTF says a skinned mesh node's own transform is ignored; three.js (the
/// engine behind `<model-viewer>`) multiplies it in anyway when binding, so a
/// file whose skinned node carries a transform renders differently in the
/// product's own viewer than the spec says. Moving the mesh to a fresh root
/// with identity transform changes nothing under the spec and makes every
/// reader agree. The original node keeps its name, transform and children,
/// and the light or camera it places.
pub fn hoist_skinned_meshes(scene: &Scene) -> Scene {
    let mut out = scene.clone();
    let world = scene.world_matrices();
    let parents = scene.parents();
    // A mesh mixing skinned and rigid primitives is split once, however many
    // nodes draw it: the rigid part stays on the node, which still places
    // it, and only the skinned part moves (`pose::world_primitives` draws a
    // primitive without weights by its node even under a skin; glTF has no
    // such primitive, as every primitive under `node.skin` is skinned).
    let mut parts: HashMap<usize, (Option<usize>, Option<usize>)> = HashMap::new();
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        let (Some(mesh), Some(skin)) = (n.mesh, n.skin) else { continue };
        let (rigid, skinned) = *parts.entry(mesh).or_insert_with(|| {
            let prims = &scene.meshes[mesh].primitives;
            match prims.iter().filter(|p| p.skin.is_some()).count() {
                0 => (Some(mesh), None),
                k if k == prims.len() => (None, Some(mesh)),
                _ => {
                    let mut part = |skinned: bool| {
                        let mut m = scene.meshes[mesh].clone();
                        m.primitives.retain(|p| p.skin.is_some() == skinned);
                        out.meshes.push(m);
                        out.meshes.len() - 1
                    };
                    (Some(part(false)), Some(part(true)))
                }
            }
        });
        let Some(skinned) = skinned else {
            // No weights at all: an ordinary rigid mesh.
            out.nodes[i].skin = None;
            continue;
        };
        let at_identity_root = parents[i].is_none()
            && world[i].iter().zip(math::IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-12);
        if at_identity_root && rigid.is_none() {
            continue;
        }
        out.nodes[i].mesh = rigid;
        out.nodes[i].skin = None;
        let weights = if rigid.is_some() { out.nodes[i].weights.clone() } else { out.nodes[i].weights.take() };
        let name = n.name.clone().map(|s| format!("{s}_skinned"));
        let new = out.add_root(Node { name, mesh: Some(skinned), skin: Some(skin), weights, ..Default::default() });
        // Morph-weight animation targets the node that holds the mesh: both
        // halves, when it was split.
        for a in out.animations.iter_mut() {
            let mut copies = Vec::new();
            for c in a.channels.iter_mut().filter(|c| c.node == i && c.path == AnimPath::Weights) {
                if rigid.is_some() {
                    copies.push(Channel { node: new, ..c.clone() });
                } else {
                    c.node = new;
                }
            }
            a.channels.extend(copies);
        }
    }
    out
}

/// Primitives that draw nothing, meshes left with none, and meshes no node
/// draws (what baking a shear or splitting a skinned mesh leaves behind):
/// glTF requires at least one primitive per mesh and one element per
/// accessor, and an unused mesh is only its geometry written twice, so none
/// is written. Nodes that drew an empty mesh draw nothing (no mesh, so no
/// skin, weights or weight animation either, which each require one).
fn drop_empty_meshes(s: &mut Scene) {
    // A primitive draws nothing when it has fewer indices than one of its own
    // elements takes: three for a triangle, two for a segment, one for a point.
    let empty = |p: &Primitive| p.positions.is_empty() || p.indices.len() < p.topology.indices_per_element();
    let mut used = vec![false; s.meshes.len()];
    for m in s.nodes.iter().filter_map(|n| n.mesh) {
        if let Some(u) = used.get_mut(m) {
            *u = true;
        }
    }
    if used.iter().all(|u| *u) && !s.meshes.iter().any(|m| m.primitives.is_empty() || m.primitives.iter().any(empty)) {
        return;
    }
    let mut remap = vec![None; s.meshes.len()];
    let mut kept = Vec::with_capacity(s.meshes.len());
    for (i, mut m) in std::mem::take(&mut s.meshes).into_iter().enumerate() {
        m.primitives.retain(|p| !empty(p));
        if used[i] && !m.primitives.is_empty() {
            remap[i] = Some(kept.len());
            kept.push(m);
        }
    }
    s.meshes = kept;
    for n in s.nodes.iter_mut() {
        n.mesh = n.mesh.and_then(|m| remap.get(m).copied().flatten());
        if n.mesh.is_none() {
            n.skin = None;
            n.weights = None;
        }
    }
    let nodes = &s.nodes;
    for a in s.animations.iter_mut() {
        a.channels.retain(|c| c.path != AnimPath::Weights || nodes.get(c.node).is_some_and(|n| n.mesh.is_some()));
    }
    s.animations.retain(|a| !a.channels.is_empty());
}

/// What the glTF writer changes beyond the capability table: shear it
/// cannot say ([`pose::bake_shear`]), and clips moved to start at 0.
pub fn losses(scene: &Scene, target: &str) -> Vec<super::scene::Loss> {
    use super::scene::{Loss, LossKind};
    let mut out = super::pose::shear_losses(scene, target);
    let early: Vec<String> = scene
        .animations
        .iter()
        .enumerate()
        .filter(|(_, a)| a.channels.iter().any(|c| c.times.first().is_some_and(|t| *t < 0.0)))
        .map(|(i, a)| a.name.clone().unwrap_or_else(|| format!("clip {i}")))
        .collect();
    if let Some(first) = early.first() {
        out.push(Loss::new(
            LossKind::Animation,
            format!("the start time of {} clip(s) keyed before 0 (first: {first:?})", early.len()),
            format!("{target} key times start at 0; each clip is shifted whole to start there"),
        ));
    }
    out
}

/// Each node's depth and its 2^k-th ancestors (binary lifting) in the written
/// graph, for the `skeleton` of each skin. glTF 2.0 §5.28.2: "The node MUST be
/// the closest common root of the joints hierarchy or a direct or indirect
/// parent node of the closest common root." A skin's closest common root is
/// the pairwise fold of its joints at O(log depth) each, so a document of
/// many skins on a deep chain stays near linear. It is built once per
/// document that has a skin, from [`Scene::parents`] of the scene being
/// written: `build_document`'s `owned`, the scene it validated
/// ([`Scene::validate`]) with nodes only appended, by the hoist
/// ([`hoist_skinned_meshes`]) and `pose::bake_shear`. That is still a forest
/// with every index in range and no cycle, of at most 3 × `MAX_NODES`
/// (100,000) nodes, and no deeper than `MAX_NODES`. Entries are u32, since
/// every index still fits one, and the table is half a usize one: 17 levels
/// at the extreme, about 20 MB.
struct Ancestry {
    depth: Vec<u32>,
    // `up[k][n]`: n's ancestor 2^k levels up, or its root when that is nearer.
    up: Vec<Vec<u32>>,
}

impl Ancestry {
    fn new(parents: &[Option<usize>]) -> Ancestry {
        // Depths from the parent table itself: climb from each node to one
        // whose depth is known, or to a root, then set them on the way down.
        let mut depth = vec![u32::MAX; parents.len()];
        let mut climb = Vec::new();
        for start in 0..parents.len() {
            let mut n = start;
            while depth[n] == u32::MAX {
                climb.push(n);
                match parents[n] {
                    Some(p) => n = p,
                    None => break,
                }
            }
            while let Some(n) = climb.pop() {
                depth[n] = parents[n].map_or(0, |p| depth[p] + 1);
            }
        }
        // ⌈log2(max depth + 1)⌉ levels reach any ancestor, and `common` needs
        // level 0 even when every node is a root. A root is its own parent.
        let max = depth.iter().copied().max().unwrap_or(0);
        let levels = (u32::BITS - max.leading_zeros()).max(1) as usize;
        let mut up: Vec<Vec<u32>> = vec![parents.iter().enumerate().map(|(n, p)| p.unwrap_or(n) as u32).collect()];
        for k in 1..levels {
            let below = &up[k - 1];
            let next = below.iter().map(|&a| below[a as usize]).collect();
            up.push(next);
        }
        Ancestry { depth, up }
    }

    // n's ancestor `by` levels up; `by` is at most n's depth.
    fn lift(&self, mut n: usize, mut by: u32) -> usize {
        let mut k = 0;
        while by > 0 {
            if by & 1 == 1 {
                n = self.up[k][n] as usize;
            }
            by >>= 1;
            k += 1;
        }
        n
    }

    // The deepest node that is a or above it and b or above it; `None` when
    // they sit in different trees.
    fn common(&self, a: usize, b: usize) -> Option<usize> {
        let (da, db) = (self.depth[a], self.depth[b]);
        let (mut a, mut b) = if da > db { (self.lift(a, da - db), b) } else { (a, self.lift(b, db - da)) };
        if a == b {
            return Some(a);
        }
        // Climb both while they stay apart: they end just below their common
        // root, or at the roots of two trees.
        for level in self.up.iter().rev() {
            let (x, y) = (level[a] as usize, level[b] as usize);
            if x != y {
                (a, b) = (x, y);
            }
        }
        let (x, y) = (self.up[0][a], self.up[0][b]);
        (x == y).then_some(x as usize)
    }

    // A skin's closest common root: `None` for no joints, and as soon as two
    // joints sit in different trees.
    fn closest_common_root(&self, joints: &[usize]) -> Option<usize> {
        let (&first, rest) = joints.split_first()?;
        rest.iter().try_fold(first, |root, &j| self.common(root, j))
    }

    // The `skeleton` a skin is written with: the one its scene holds when that
    // is the closest common root or above it (so above every joint), and the
    // closest common root otherwise.
    fn skeleton(&self, held: Option<usize>, joints: &[usize]) -> Option<usize> {
        let root = self.closest_common_root(joints)?;
        let above = |k: usize| self.depth[k] <= self.depth[root] && self.lift(root, self.depth[root] - self.depth[k]) == k;
        Some(held.filter(|&k| above(k)).unwrap_or(root))
    }
}

fn build_document(scene: &Scene, placement: ImagePlacement) -> Result<Written, MeshError> {
    scene.validate()?;
    let mut owned = hoist_skinned_meshes(scene);
    // Only valid documents: every local matrix decomposes (§5.25), no key
    // time is negative (§3.11), no mesh is empty (§5.23's minItems).
    super::pose::bake_shear(&mut owned);
    drop_empty_meshes(&mut owned);
    for a in owned.animations.iter_mut() {
        super::pose::start_at_zero(&mut a.channels);
    }
    let scene = &owned;
    let mut b = Builder { bin: Vec::new(), views: Vec::new(), accessors: Vec::new() };
    let mut extensions_used: Vec<&'static str> = Vec::new();
    let mut extensions_required: Vec<&'static str> = Vec::new();
    let mut image_files: Vec<PackageFile> = Vec::new();

    // Meshes. POSITION of the first primitive is always accessor 0 — the flat
    // writer's long-standing layout, which its tests pin.
    let mut meshes_json = Vec::with_capacity(scene.meshes.len());
    for mesh in &scene.meshes {
        let mut prims_json = Vec::new();
        for p in &mesh.primitives {
            let mut attrs = serde_json::Map::new();
            let flat: Vec<f32> = p.positions.iter().flatten().copied().collect();
            attrs.insert("POSITION".into(), Value::from(b.floats(&flat, 3, Some(TARGET_ARRAY_BUFFER), true)));
            if let Some(ns) = &p.normals {
                // glTF requires unit normals; a zero one has no direction to
                // keep, so it takes the geometry's.
                // Only a triangle has a facet to take it from: a segment's or
                // a point's takes +Z, not the normal of whichever three of its
                // indices come first.
                let facets: &[u32] = if p.topology == super::scene::Topology::Triangles { &p.indices } else { &[] };
                let derived = super::pose::derived_normals(&p.positions, facets);
                let flat: Vec<f32> = ns
                    .iter()
                    .zip(&derived)
                    .flat_map(|(n, d)| unit(*n).unwrap_or(*d))
                    .collect();
                attrs.insert("NORMAL".into(), Value::from(b.floats(&flat, 3, Some(TARGET_ARRAY_BUFFER), false)));
            }
            if let Some(ts) = &p.tangents {
                let flat: Vec<f32> = ts
                    .iter()
                    .flat_map(|t| {
                        let xyz = unit([t[0], t[1], t[2]]).unwrap_or([1.0, 0.0, 0.0]);
                        [xyz[0], xyz[1], xyz[2], if t[3] < 0.0 { -1.0 } else { 1.0 }]
                    })
                    .collect();
                attrs.insert("TANGENT".into(), Value::from(b.floats(&flat, 4, Some(TARGET_ARRAY_BUFFER), false)));
            }
            for (si, set) in p.uvs.iter().enumerate() {
                let flat: Vec<f32> = set.iter().flatten().copied().collect();
                attrs.insert(format!("TEXCOORD_{si}"), Value::from(b.floats(&flat, 2, Some(TARGET_ARRAY_BUFFER), false)));
            }
            if let Some(cs) = &p.colors {
                let flat: Vec<f32> = cs.iter().flatten().map(|c| c.clamp(0.0, 1.0)).collect();
                attrs.insert("COLOR_0".into(), Value::from(b.floats(&flat, 4, Some(TARGET_ARRAY_BUFFER), false)));
            }
            if let Some(sw) = &p.skin {
                let sets = sw.influences / 4;
                let mut normalized: Vec<f32> = Vec::with_capacity(sw.weights.len());
                for v in 0..p.positions.len() {
                    let (_, ws) = sw.vertex(v);
                    normalized.extend(normalized_weights(ws));
                }
                for s in 0..sets {
                    let mut js = Vec::with_capacity(p.positions.len() * 4);
                    let mut ws = Vec::with_capacity(p.positions.len() * 4);
                    for v in 0..p.positions.len() {
                        for k in 0..4 {
                            let at = v * sw.influences + s * 4 + k;
                            let w = normalized[at];
                            // An unused slot names joint 0 with weight 0, which
                            // is what the validator expects of padding.
                            js.push(if w > 0.0 { sw.joints[at] } else { 0 });
                            ws.push(w);
                        }
                    }
                    attrs.insert(format!("JOINTS_{s}"), Value::from(b.u16s(&js, 4, Some(TARGET_ARRAY_BUFFER))));
                    attrs.insert(format!("WEIGHTS_{s}"), Value::from(b.floats(&ws, 4, Some(TARGET_ARRAY_BUFFER), false)));
                }
            }
            let mut prim = serde_json::Map::new();
            prim.insert("attributes".into(), Value::Object(attrs));
            prim.insert("indices".into(), Value::from(b.indices(&p.indices)));
            // Triangles are the default mode (4) and are left unwritten.
            match p.topology {
                super::scene::Topology::Triangles => {}
                super::scene::Topology::Lines => {
                    prim.insert("mode".into(), Value::from(MODE_LINES));
                }
                super::scene::Topology::Points => {
                    prim.insert("mode".into(), Value::from(MODE_POINTS));
                }
            }
            if let Some(m) = p.material {
                prim.insert("material".into(), Value::from(m));
            }
            if !p.targets.is_empty() {
                let mut targets = Vec::new();
                for t in &p.targets {
                    let mut tj = serde_json::Map::new();
                    let flat: Vec<f32> = t.positions.iter().flatten().copied().collect();
                    tj.insert("POSITION".into(), Value::from(b.floats(&flat, 3, Some(TARGET_ARRAY_BUFFER), true)));
                    if let Some(ns) = &t.normals {
                        let flat: Vec<f32> = ns.iter().flatten().copied().collect();
                        tj.insert("NORMAL".into(), Value::from(b.floats(&flat, 3, Some(TARGET_ARRAY_BUFFER), false)));
                    }
                    targets.push(Value::Object(tj));
                }
                prim.insert("targets".into(), Value::Array(targets));
            }
            prims_json.push(Value::Object(prim));
        }
        let mut mj = serde_json::Map::new();
        mj.insert("primitives".into(), Value::Array(prims_json));
        if let Some(n) = &mesh.name {
            mj.insert("name".into(), Value::from(n.clone()));
        }
        if !mesh.weights.is_empty() {
            mj.insert("weights".into(), serde_json::json!(mesh.weights));
        }
        if !mesh.target_names.is_empty() {
            mj.insert("extras".into(), serde_json::json!({ "targetNames": mesh.target_names }));
        }
        meshes_json.push(Value::Object(mj));
    }

    // Images and textures. A texture whose image is not PNG/JPEG still goes in
    // under its extension, with that extension required: there is no fallback
    // to point `source` at.
    let mut images_json = Vec::new();
    let mut used_names: Vec<String> = Vec::new();
    for (ii, im) in scene.images.iter().enumerate() {
        let mut ij = serde_json::Map::new();
        match placement {
            ImagePlacement::BufferView => {
                let view = b.view(&im.data, None);
                ij.insert("bufferView".into(), Value::from(view));
                ij.insert("mimeType".into(), Value::from(im.format.mime()));
            }
            ImagePlacement::Files => {
                let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("image_{ii}"));
                let mut name = format!("{stem}.{}", im.format.extension());
                let mut n = 1;
                // Folded: paths that differ only in case are one file on macOS and Windows.
                while used_names.contains(&name.to_ascii_lowercase()) || name.eq_ignore_ascii_case("model.bin") {
                    name = format!("{stem}_{n}.{}", im.format.extension());
                    n += 1;
                }
                used_names.push(name.to_ascii_lowercase());
                ij.insert("uri".into(), Value::from(name.clone()));
                image_files.push(PackageFile { path: name, bytes: im.data.clone() });
            }
        }
        if let Some(n) = &im.name {
            ij.insert("name".into(), Value::from(n.clone()));
        }
        images_json.push(Value::Object(ij));
    }
    let mut samplers_json: Vec<Value> = Vec::new();
    let mut textures_json = Vec::new();
    let mut tex_map: Vec<Option<usize>> = Vec::new();
    for t in &scene.textures {
        let s = &t.sampler;
        let mut sj = serde_json::Map::new();
        let wrap = |w: Wrap| match w {
            Wrap::Repeat => 10497,
            Wrap::ClampToEdge => 33071,
            Wrap::MirroredRepeat => 33648,
        };
        if s.wrap_s != Wrap::Repeat {
            sj.insert("wrapS".into(), Value::from(wrap(s.wrap_s)));
        }
        if s.wrap_t != Wrap::Repeat {
            sj.insert("wrapT".into(), Value::from(wrap(s.wrap_t)));
        }
        if let Some(f) = s.mag_filter {
            sj.insert("magFilter".into(), Value::from(if f == Filter::Nearest { 9728 } else { 9729 }));
        }
        if let Some(f) = s.min_filter {
            let v = match (f, s.mipmaps, s.mipmap_filter) {
                (Filter::Nearest, false, _) => 9728,
                (Filter::Linear, false, _) => 9729,
                (Filter::Nearest, true, Filter::Nearest) => 9984,
                (Filter::Linear, true, Filter::Nearest) => 9985,
                (Filter::Nearest, true, Filter::Linear) => 9986,
                (Filter::Linear, true, Filter::Linear) => 9987,
            };
            sj.insert("minFilter".into(), Value::from(v));
        }
        let sj = Value::Object(sj);
        let sampler = match samplers_json.iter().position(|x| *x == sj) {
            Some(i) => i,
            None => {
                samplers_json.push(sj);
                samplers_json.len() - 1
            }
        };
        let fmt = scene.images[t.image].format;
        let mut tj = serde_json::json!({ "sampler": sampler });
        match fmt {
            ImageFormat::Png | ImageFormat::Jpeg => tj["source"] = Value::from(t.image),
            ImageFormat::Webp => {
                tj["extensions"] = serde_json::json!({ "EXT_texture_webp": { "source": t.image } });
                for list in [&mut extensions_used, &mut extensions_required] {
                    if !list.contains(&"EXT_texture_webp") {
                        list.push("EXT_texture_webp");
                    }
                }
            }
            ImageFormat::Ktx2 => {
                tj["extensions"] = serde_json::json!({ "KHR_texture_basisu": { "source": t.image } });
                for list in [&mut extensions_used, &mut extensions_required] {
                    if !list.contains(&"KHR_texture_basisu") {
                        list.push("KHR_texture_basisu");
                    }
                }
            }
        }
        if let Some(n) = &t.name {
            tj["name"] = Value::from(n.clone());
        }
        textures_json.push(tj);
        tex_map.push(Some(textures_json.len() - 1));
    }

    // Materials.
    let mut materials_json = Vec::new();
    for (mi, m) in scene.materials.iter().enumerate() {
        let finite = m.base_color.iter().chain(&m.emissive).all(|v| v.is_finite());
        if !finite {
            return Err(malformed(format!("material {mi} base_color or emissive is not finite")));
        }
        let mut pbr = serde_json::Map::new();
        if m.base_color != [1.0; 4] {
            pbr.insert("baseColorFactor".into(), serde_json::json!(m.base_color));
        }
        if let Some(t) = m.base_color_texture.as_ref().and_then(|r| texture_info_json(r, &tex_map, &mut extensions_used)) {
            pbr.insert("baseColorTexture".into(), t);
        }
        // Written explicitly: the spec defaults are 1.0, which is a mirror,
        // and a reader that misses the default would show something else.
        pbr.insert("metallicFactor".into(), Value::from(m.metallic));
        pbr.insert("roughnessFactor".into(), Value::from(m.roughness));
        if let Some(t) = m.metallic_roughness_texture.as_ref().and_then(|r| texture_info_json(r, &tex_map, &mut extensions_used)) {
            pbr.insert("metallicRoughnessTexture".into(), t);
        }
        let mut mj = serde_json::Map::new();
        if let Some(n) = &m.name {
            mj.insert("name".into(), Value::from(n.clone()));
        }
        mj.insert("pbrMetallicRoughness".into(), Value::Object(pbr));
        if let Some(mut t) = m.normal_texture.as_ref().and_then(|r| texture_info_json(r, &tex_map, &mut extensions_used)) {
            if m.normal_scale != 1.0 {
                t["scale"] = Value::from(m.normal_scale);
            }
            mj.insert("normalTexture".into(), t);
        }
        if let Some(mut t) = m.occlusion_texture.as_ref().and_then(|r| texture_info_json(r, &tex_map, &mut extensions_used)) {
            if m.occlusion_strength != 1.0 {
                t["strength"] = Value::from(m.occlusion_strength);
            }
            mj.insert("occlusionTexture".into(), t);
        }
        if let Some(t) = m.emissive_texture.as_ref().and_then(|r| texture_info_json(r, &tex_map, &mut extensions_used)) {
            mj.insert("emissiveTexture".into(), t);
        }
        let mut exts = serde_json::Map::new();
        if m.emissive != [0.0; 3] {
            // emissiveFactor is clamped to [0, 1] by the schema; anything
            // brighter rides in KHR_materials_emissive_strength.
            let peak = m.emissive.iter().cloned().fold(0.0f32, f32::max);
            if peak > 1.0 {
                mj.insert("emissiveFactor".into(), serde_json::json!(m.emissive.map(|c| c / peak)));
                exts.insert("KHR_materials_emissive_strength".into(), serde_json::json!({ "emissiveStrength": peak }));
                if !extensions_used.contains(&"KHR_materials_emissive_strength") {
                    extensions_used.push("KHR_materials_emissive_strength");
                }
            } else {
                mj.insert("emissiveFactor".into(), serde_json::json!(m.emissive));
            }
        }
        match m.alpha_mode {
            AlphaMode::Opaque => {}
            AlphaMode::Mask => {
                mj.insert("alphaMode".into(), Value::from("MASK"));
                if m.alpha_cutoff != 0.5 {
                    mj.insert("alphaCutoff".into(), Value::from(m.alpha_cutoff));
                }
            }
            AlphaMode::Blend => {
                mj.insert("alphaMode".into(), Value::from("BLEND"));
            }
        }
        if m.double_sided {
            mj.insert("doubleSided".into(), Value::from(true));
        }
        if m.unlit {
            exts.insert("KHR_materials_unlit".into(), serde_json::json!({}));
            if !extensions_used.contains(&"KHR_materials_unlit") {
                extensions_used.push("KHR_materials_unlit");
            }
        }
        write_layer_extensions(m, &tex_map, &mut extensions_used, &mut exts);
        if !exts.is_empty() {
            mj.insert("extensions".into(), Value::Object(exts));
        }
        materials_json.push(Value::Object(mj));
    }

    // Skins. One whose scene names no skeleton (every FBX source, and
    // multi-root USD), or names one that is no common root of its joints,
    // gets the joints' closest common root, found in the graph the nodes
    // below are written from: this one, after the hoist. The table is built
    // at the first skin.
    let mut ancestry: Option<Ancestry> = None;
    let mut skins_json = Vec::new();
    for s in &scene.skins {
        let flat: Vec<f32> = s.inverse_bind_matrices.iter().flatten().map(|v| *v as f32).collect();
        let acc = b.floats(&flat, 16, None, false);
        let mut sj = serde_json::json!({ "joints": s.joints, "inverseBindMatrices": acc });
        let roots = ancestry.get_or_insert_with(|| Ancestry::new(&scene.parents()));
        if let Some(k) = roots.skeleton(s.skeleton, &s.joints) {
            sj["skeleton"] = Value::from(k);
        }
        if let Some(n) = &s.name {
            sj["name"] = Value::from(n.clone());
        }
        skins_json.push(sj);
    }

    // Nodes.
    let mut nodes_json = Vec::new();
    for n in &scene.nodes {
        let mut nj = serde_json::Map::new();
        if let Some(name) = &n.name {
            nj.insert("name".into(), Value::from(name.clone()));
        }
        match &n.transform {
            Transform::Trs { translation, rotation, scale } => {
                if *translation != [0.0; 3] {
                    nj.insert("translation".into(), serde_json::json!(translation));
                }
                if *rotation != math::QUAT_IDENTITY {
                    nj.insert("rotation".into(), serde_json::json!(math::quat_normalize(*rotation)));
                }
                if *scale != [1.0; 3] {
                    nj.insert("scale".into(), serde_json::json!(scale));
                }
            }
            Transform::Matrix(m) => {
                nj.insert("matrix".into(), serde_json::json!(m));
            }
        }
        if !n.children.is_empty() {
            nj.insert("children".into(), serde_json::json!(n.children));
        }
        if let Some(m) = n.mesh {
            nj.insert("mesh".into(), Value::from(m));
        }
        if let Some(s) = n.skin {
            nj.insert("skin".into(), Value::from(s));
        }
        if let Some(w) = &n.weights {
            nj.insert("weights".into(), serde_json::json!(w));
        }
        if let Some(c) = n.camera {
            nj.insert("camera".into(), Value::from(c));
        }
        if let Some(l) = n.light {
            nj.insert("extensions".into(), serde_json::json!({ "KHR_lights_punctual": { "light": l } }));
        }
        nodes_json.push(Value::Object(nj));
    }
    // Each light and camera once, placed by index from every node above
    // that holds it.
    let lights_json: Vec<Value> = scene.lights.iter().map(light_json).collect();
    let cameras_json: Vec<Value> = scene.cameras.iter().map(camera_json).collect();

    // Animations: one sampler per channel, input accessors shared when the key
    // times are identical (the common "every channel keyed on every frame").
    let mut animations_json = Vec::new();
    // Keyed by the times' bits: scanning every earlier accessor per channel
    // was quadratic in the channels whose times differ.
    let mut time_accessors: HashMap<Vec<u32>, usize> = HashMap::new();
    for a in &scene.animations {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for ch in &a.channels {
            let key: Vec<u32> = ch.times.iter().map(|t| t.to_bits()).collect();
            let input = match time_accessors.get(&key) {
                Some(acc) => *acc,
                None => {
                    let acc = b.floats(&ch.times, 1, None, true);
                    time_accessors.insert(key, acc);
                    acc
                }
            };
            let output = match ch.path {
                AnimPath::Translation | AnimPath::Scale => b.floats(&ch.values, 3, None, false),
                AnimPath::Rotation => b.floats(&ch.values, 4, None, false),
                AnimPath::Weights => b.floats(&ch.values, 1, None, false),
            };
            let interp = match ch.interpolation {
                Interpolation::Linear => "LINEAR",
                Interpolation::Step => "STEP",
                Interpolation::CubicSpline => "CUBICSPLINE",
            };
            samplers.push(serde_json::json!({ "input": input, "output": output, "interpolation": interp }));
            channels.push(serde_json::json!({
                "sampler": samplers.len() - 1,
                "target": { "node": ch.node, "path": ch.path.as_str() }
            }));
        }
        let mut aj = serde_json::json!({ "samplers": samplers, "channels": channels });
        if let Some(n) = &a.name {
            aj["name"] = Value::from(n.clone());
        }
        animations_json.push(aj);
    }

    pad_to_4(&mut b.bin, 0);
    let mut doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "litegen mesh converter" },
        "scene": 0,
        "scenes": [ { "nodes": scene.roots } ],
        "nodes": nodes_json,
    });
    let obj = doc.as_object_mut().ok_or_else(|| malformed("glTF document is not an object"))?;
    if let Some(n) = &scene.name {
        obj.insert("scenes".into(), serde_json::json!([{ "name": n, "nodes": scene.roots }]));
    }
    // Each top-level array is inserted only when non-empty: glTF forbids empty
    // arrays, which the official validator flags even though loaders shrug.
    let mut put = |k: &str, v: Vec<Value>| {
        if !v.is_empty() {
            obj.insert(k.into(), Value::Array(v));
        }
    };
    put("meshes", meshes_json);
    put("materials", materials_json);
    put("textures", textures_json);
    put("images", images_json);
    put("samplers", samplers_json);
    put("skins", skins_json);
    put("animations", animations_json);
    put("cameras", cameras_json);
    put("accessors", std::mem::take(&mut b.accessors));
    put("bufferViews", std::mem::take(&mut b.views));
    if !b.bin.is_empty() {
        obj.insert("buffers".into(), serde_json::json!([{ "byteLength": b.bin.len() }]));
    }
    // In `extensionsUsed` only, never required: a reader without the
    // extension draws the model by its own light, which is its intent.
    if !lights_json.is_empty() {
        obj.insert("extensions".into(), serde_json::json!({ "KHR_lights_punctual": { "lights": lights_json } }));
        extensions_used.push("KHR_lights_punctual");
    }
    if !extensions_used.is_empty() {
        obj.insert("extensionsUsed".into(), serde_json::json!(extensions_used));
    }
    if !extensions_required.is_empty() {
        obj.insert("extensionsRequired".into(), serde_json::json!(extensions_required));
    }
    Ok(Written { doc, bin: b.bin, image_files })
}

/// Write a scene as a glTF binary.
pub fn write_scene(scene: &Scene) -> Result<Vec<u8>, MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let w = build_document(scene, ImagePlacement::BufferView)?;
    let mut json = serde_json::to_vec(&w.doc)
        .map_err(|e| malformed(format!("could not serialise the glTF JSON: {e}")))?;
    // BIN pads with zeros, JSON with spaces — both are spec requirements and
    // viewers do reject violations.
    pad_to_4(&mut json, b' ');
    let mut bin = w.bin;
    pad_to_4(&mut bin, 0);
    let bin_chunk = if bin.is_empty() { 0 } else { 8 + bin.len() };
    let total = HEADER_LEN + 8 + json.len() + bin_chunk;
    if total > u32::MAX as usize {
        return Err(unsupported(format!(
            "{total} bytes exceeds the 4 GiB a GLB header can declare"
        )));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&bin);
    }
    Ok(out)
}

/// Write a scene as a `.gltf` JSON document.
///
/// `self_contained` embeds the buffer (and with it every image) as a base64
/// data URI, so the one file is the whole model — what a caller that can store
/// only one file needs. Otherwise the package is `model.gltf` + `model.bin` +
/// one file per image, the layout DCC tools call "glTF separate".
pub fn write_gltf_scene(scene: &Scene, self_contained: bool) -> Result<Package, MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    let placement = if self_contained { ImagePlacement::BufferView } else { ImagePlacement::Files };
    let mut w = build_document(scene, placement)?;
    let mut files = Vec::new();
    if !w.bin.is_empty() {
        let uri = if self_contained {
            format!("data:application/octet-stream;base64,{}", encode_base64(&w.bin))
        } else {
            "model.bin".to_string()
        };
        w.doc["buffers"] = serde_json::json!([{ "byteLength": w.bin.len(), "uri": uri }]);
        if !self_contained {
            files.push(PackageFile { path: "model.bin".into(), bytes: w.bin });
        }
    }
    let json = serde_json::to_vec_pretty(&w.doc)
        .map_err(|e| malformed(format!("could not serialise the glTF JSON: {e}")))?;
    let mut out = vec![PackageFile { path: "model.gltf".into(), bytes: json }];
    out.extend(files);
    out.extend(w.image_files);
    Ok(Package { files: out })
}

/// Write the flat mesh as a single-node, single-primitive GLB.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // `super::write` validates first, but this is a public entry point — re-check
    // rather than trust the caller.
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    if let Some(c) = mesh.base_color {
        for (i, v) in c.iter().enumerate() {
            if !v.is_finite() {
                return Err(malformed(format!("base_color[{i}] is {v}")));
            }
        }
    }
    // glTF REQUIRES min/max on a POSITION accessor, and three.js throws outright
    // without them. A NaN coordinate would serialise as JSON `null` and produce
    // a file that parses and then renders nothing.
    for (i, p) in mesh.positions.iter().enumerate() {
        for a in 0..3 {
            if !p[a].is_finite() {
                return Err(malformed(format!("position {i} component {a} is {}", p[a])));
            }
        }
    }
    write_scene(&super::pose::from_flat(mesh))
}

// ─── tests ──────────────────────────────────────────────────────────────────

/// The glTF document a GLB holds, for tests outside this file (serde_json
/// stays here: `isolation_tests`).
#[cfg(test)]
pub(super) fn json_chunk(glb: &[u8]) -> Value {
    let len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&glb[20..20 + len]).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures::lightcam;
    use super::super::fixtures::sweeps::{layer_sweep, lean_layered, span};

    // ── builders ────────────────────────────────────────────────────────────

    fn f32s(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }
    fn u16s(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }
    fn u32s(v: &[u32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    /// Assemble a spec-shaped GLB around a JSON document and an optional BIN.
    fn pack(doc: Value, bin: Option<&[u8]>) -> Vec<u8> {
        let mut json = serde_json::to_vec(&doc).unwrap();
        pad_to_4(&mut json, b' ');
        let bin = bin.map(|b| {
            let mut v = b.to_vec();
            pad_to_4(&mut v, 0);
            v
        });
        let total = 12 + 8 + json.len() + bin.as_ref().map_or(0, |b| 8 + b.len());
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        out.extend_from_slice(&json);
        if let Some(b) = &bin {
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
            out.extend_from_slice(b);
        }
        out
    }

    /// A single triangle: 3 positions then 3 u32 indices, with the JSON that
    /// describes it. Tests mutate the returned document before packing.
    fn triangle_doc() -> (Value, Vec<u8>) {
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                 "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        (doc, bin)
    }

    fn triangle_glb() -> Vec<u8> {
        let (doc, bin) = triangle_doc();
        pack(doc, Some(&bin))
    }

    fn sample_mesh() -> Mesh {
        Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.5]],
            indices: vec![0, 1, 2, 1, 3, 2],
            normals: None,
            uvs: None,
            base_color: None,
            name: None,
        }
    }

    fn json_of(glb: &[u8]) -> Value {
        let len = u32_le(glb, 12).unwrap() as usize;
        serde_json::from_slice(&glb[20..20 + len]).expect("writer emits parseable JSON")
    }

    /// `scene` written as GLB with its JSON edited by `f`.
    fn patched(scene: &Scene, f: impl FnOnce(&mut Value)) -> Vec<u8> {
        let glb = write_scene(scene).unwrap();
        let mut doc = json_of(&glb);
        let json_len = u32_le(&glb, 12).unwrap() as usize;
        let bin_at = 20 + json_len;
        let bin_len = u32_le(&glb, bin_at).unwrap() as usize;
        f(&mut doc);
        pack(doc, Some(&glb[bin_at + 8..bin_at + 8 + bin_len]))
    }

    /// 2026-10-12 review, finding 27: a finite JSON number past f32's range
    /// (1e39) became infinity on the way into the scene, and NaN in every
    /// position it touched.
    #[test]
    fn numbers_past_a_32_bit_float_are_refused() {
        let s = super::super::fixtures::morph();
        for (what, edit) in [
            ("mesh weights", Box::new(|d: &mut Value| d["meshes"][0]["weights"] = serde_json::json!([1e39, 0.0])) as Box<dyn Fn(&mut Value)>),
            ("node weights", Box::new(|d: &mut Value| d["nodes"][0]["weights"] = serde_json::json!([1e39, 0.0]))),
            ("metallicFactor", Box::new(|d: &mut Value| d["materials"][0]["pbrMetallicRoughness"] = serde_json::json!({"metallicFactor": 1e39}))),
            ("translation", Box::new(|d: &mut Value| d["nodes"][0]["translation"] = serde_json::json!([1e39, 0.0, 0.0]))),
        ] {
            let bytes = patched(&s, |d| edit(d));
            match read_scene(&bytes, &NoSidecars) {
                Err(MeshError::Malformed(m)) => assert!(m.contains("32-bit"), "{what}: {m}"),
                Err(e) => panic!("{what}: {e}"),
                Ok(r) => panic!("{what}: read as {:?}", r.scene.nodes[0]),
            }
        }
    }

    fn unwrap_malformed(e: MeshError) -> String {
        match e {
            MeshError::Malformed(d) => d,
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    fn unwrap_unsupported(e: MeshError) -> String {
        match e {
            MeshError::Unsupported(d) => d,
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    // ── round trips ─────────────────────────────────────────────────────────

    #[test]
    fn round_trips_a_bare_mesh() {
        let mesh = sample_mesh();
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back, mesh);
    }

    #[test]
    fn round_trips_every_optional_attribute() {
        let mut mesh = sample_mesh();
        mesh.normals = Some(vec![[0.0, 0.0, 1.0]; 4]);
        mesh.uvs = Some(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        mesh.base_color = Some([0.25, 0.5, 0.75, 1.0]);
        mesh.name = Some("widget".into());
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back, mesh);
    }

    #[test]
    fn round_trips_normals_without_uvs() {
        let mut mesh = sample_mesh();
        mesh.normals = Some(vec![[0.0, 1.0, 0.0]; 4]);
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.normals, mesh.normals);
        assert_eq!(back.uvs, None, "absent uvs must stay absent");
    }

    #[test]
    fn round_trips_uvs_without_normals() {
        let mut mesh = sample_mesh();
        mesh.uvs = Some(vec![[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]]);
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.uvs, mesh.uvs);
        assert_eq!(back.normals, None, "absent normals must stay absent");
    }

    #[test]
    fn round_trips_base_color_without_a_name() {
        let mut mesh = sample_mesh();
        mesh.base_color = Some([0.0, 1.0, 0.0, 0.5]);
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.base_color, mesh.base_color);
        assert_eq!(back.name, None);
    }

    // ── writer conformance ──────────────────────────────────────────────────

    #[test]
    fn writes_a_well_formed_container() {
        let glb = write(&sample_mesh()).unwrap();
        assert_eq!(&glb[0..4], b"glTF");
        assert_eq!(u32_le(&glb, 4).unwrap(), 2);
        assert_eq!(u32_le(&glb, 8).unwrap() as usize, glb.len(), "declared length is the real one");

        let json_len = u32_le(&glb, 12).unwrap() as usize;
        assert_eq!(u32_le(&glb, 16).unwrap(), CHUNK_JSON);
        assert_eq!(json_len % 4, 0, "JSON chunk must be 4-byte aligned");
        assert_eq!(glb[20 + json_len - 1], b' ', "JSON pads with spaces");

        let bin_off = 20 + json_len;
        let bin_len = u32_le(&glb, bin_off).unwrap() as usize;
        assert_eq!(u32_le(&glb, bin_off + 4).unwrap(), CHUNK_BIN);
        assert_eq!(bin_len % 4, 0, "BIN chunk must be 4-byte aligned");
        assert_eq!(bin_off + 8 + bin_len, glb.len(), "no trailing bytes");
    }

    #[test]
    fn position_accessor_carries_min_and_max() {
        let doc = json_of(&write(&sample_mesh()).unwrap());
        let pos = &doc["accessors"][0];
        assert_eq!(pos["min"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(pos["max"], serde_json::json!([1.0, 1.0, 0.5]));
    }

    #[test]
    fn omits_materials_when_there_is_no_base_color() {
        let doc = json_of(&write(&sample_mesh()).unwrap());
        assert!(doc.get("materials").is_none(), "empty arrays are invalid glTF");
        assert!(doc["meshes"][0]["primitives"][0].get("material").is_none());
    }

    #[test]
    fn emits_one_material_when_base_color_is_present() {
        let mut mesh = sample_mesh();
        mesh.base_color = Some([0.1, 0.2, 0.3, 1.0]);
        let doc = json_of(&write(&mesh).unwrap());
        assert_eq!(doc["materials"].as_array().unwrap().len(), 1);
        assert_eq!(doc["meshes"][0]["primitives"][0]["material"], 0);
    }

    #[test]
    fn write_rejects_an_empty_mesh() {
        assert_eq!(write(&Mesh::default()), Err(MeshError::Empty));
    }

    #[test]
    fn write_rejects_a_non_finite_position() {
        let mut mesh = sample_mesh();
        mesh.positions[1][0] = f32::NAN;
        let d = unwrap_malformed(write(&mesh).unwrap_err());
        assert!(d.contains("position 1"), "{d}");
    }

    #[test]
    fn write_rejects_a_non_finite_base_color() {
        let mut mesh = sample_mesh();
        mesh.base_color = Some([f32::INFINITY, 0.0, 0.0, 1.0]);
        let d = unwrap_malformed(write(&mesh).unwrap_err());
        assert!(d.contains("base_color[0]"), "{d}");
    }

    #[test]
    fn write_rejects_an_out_of_range_index() {
        let mut mesh = sample_mesh();
        mesh.indices = vec![0, 1, 99];
        assert!(matches!(write(&mesh), Err(MeshError::Malformed(_))));
    }

    /// The `skeleton` the first skin is written with, if any.
    fn written_skeleton(scene: &Scene) -> Option<u64> {
        json_of(&write_scene(scene).unwrap())["skins"][0].get("skeleton").and_then(Value::as_u64)
    }

    /// Leftover e of the 2026-10-30 review: a scene that names no skeleton
    /// (every FBX source, and multi-root USD) was written with none. glTF 2.0
    /// §5.28.2 allows "the closest common root of the joints hierarchy or a
    /// direct or indirect parent node of the closest common root". The writer
    /// names the closest one, joint or not, and none when the joints hang
    /// from different roots.
    #[test]
    fn glb_writes_the_closest_common_root_when_the_scene_names_no_skeleton() {
        // hip → knee → ankle, beside the tube: the root joint.
        let mut s = super::super::fixtures::skinned();
        s.skins[0].skeleton = None;
        assert_eq!(written_skeleton(&s), Some(0), "the hip");

        // That chain and a sibling joint, under an Armature that is no joint,
        // itself under a Scene node: the closest is the Armature, though the
        // Scene is a common root too.
        let mut a = s.clone();
        let twin = a.nodes.len();
        a.nodes.push(Node { name: Some("hip_twin".into()), ..Default::default() });
        let armature = a.nodes.len();
        a.nodes.push(Node { name: Some("Armature".into()), children: vec![0, twin], ..Default::default() });
        let top = a.nodes.len();
        a.nodes.push(Node { name: Some("Scene".into()), children: vec![armature], ..Default::default() });
        a.roots[0] = top;
        a.skins[0].joints.push(twin);
        a.skins[0].inverse_bind_matrices.push(math::IDENTITY);
        assert_eq!(written_skeleton(&a), Some(armature as u64), "the Armature, not the Scene above it");

        // A joint in a second root tree: no common root, so no skeleton.
        let mut t = s.clone();
        let other = t.add_root(Node { name: Some("other_root".into()), ..Default::default() });
        t.skins[0].joints.push(other);
        t.skins[0].inverse_bind_matrices.push(math::IDENTITY);
        let doc = json_of(&write_scene(&t).unwrap());
        assert!(doc["skins"][0].get("skeleton").is_none(), "two root trees: {}", doc["skins"][0]);
    }

    /// A skeleton the scene names is the file's own choice. §5.28.2 allows the
    /// closest common root or any ancestor of it, and such a one is written as
    /// it is.
    #[test]
    fn a_skeleton_the_scene_names_is_written_unchanged() {
        let s = super::super::fixtures::skinned();
        assert_eq!(s.skins[0].skeleton, Some(0));
        assert_eq!(written_skeleton(&s), Some(0));

        // The Armature above the hip, which is the joints' common root.
        let mut a = s.clone();
        let armature = a.nodes.len();
        a.nodes.push(Node { name: Some("Armature".into()), children: vec![0], ..Default::default() });
        a.roots[0] = armature;
        a.skins[0].skeleton = Some(armature);
        assert_eq!(written_skeleton(&a), Some(armature as u64), "the Armature, not the hip");
    }

    /// A held skeleton that is no common root of the joints (§5.28.2), as a
    /// vendor file can hold one: the reader keeps it after an index check, and
    /// the Khronos validator would refuse it as written. A node on a sibling
    /// branch gives way to the closest common root.
    #[test]
    fn a_held_skeleton_on_a_sibling_branch_gives_the_closest_common_root() {
        let mut s = super::super::fixtures::skinned();
        let prop = s.nodes.len();
        s.nodes.push(Node { name: Some("prop".into()), ..Default::default() });
        let armature = s.nodes.len();
        s.nodes.push(Node { name: Some("Armature".into()), children: vec![0, prop], ..Default::default() });
        s.roots[0] = armature;
        s.skins[0].skeleton = Some(prop);
        assert_eq!(written_skeleton(&s), Some(0), "the hip, not its sibling");
    }

    /// Likewise a node in another root tree.
    #[test]
    fn a_held_skeleton_in_another_root_tree_gives_the_closest_common_root() {
        let mut s = super::super::fixtures::skinned();
        let elsewhere = s.add_root(Node { name: Some("elsewhere".into()), ..Default::default() });
        s.skins[0].skeleton = Some(elsewhere);
        assert_eq!(written_skeleton(&s), Some(0), "the hip, not a node in another tree");
    }

    /// The binary-lifting table on a plain parent table, against answers
    /// worked by hand:
    ///
    /// ```text
    /// 0 (no joint)    5
    /// ├─ 1            └─ 6
    /// │  └─ 3
    /// │     └─ 4
    /// └─ 2
    /// ```
    #[test]
    fn closest_common_roots_on_a_small_parent_table() {
        let parents: Vec<Option<usize>> = vec![None, Some(0), Some(0), Some(1), Some(3), None, Some(5)];
        let t = Ancestry::new(&parents);
        let ccr = |joints: &[usize]| t.closest_common_root(joints);
        // Listed deepest first.
        assert_eq!(ccr(&[4, 3, 1]), Some(1));
        assert_eq!(ccr(&[4, 2]), Some(0));
        // Duplicates.
        assert_eq!(ccr(&[3, 3]), Some(3));
        assert_eq!(ccr(&[4, 2, 4, 2]), Some(0));
        // No joints, and one.
        assert_eq!(ccr(&[]), None);
        assert_eq!(ccr(&[4]), Some(4));
        assert_eq!(ccr(&[6]), Some(6));
        // Two separate trees, wherever the list crosses.
        assert_eq!(ccr(&[4, 6]), None);
        assert_eq!(ccr(&[6, 1, 2]), None);
        assert_eq!(ccr(&[1, 2, 6]), None);
        // A common root no list names: node 0, which is no joint.
        assert_eq!(ccr(&[3, 2]), Some(0));
        // A held skeleton stays at or above the closest common root, and is
        // replaced anywhere else.
        assert_eq!(t.skeleton(Some(3), &[4, 3]), Some(3), "the root itself");
        assert_eq!(t.skeleton(Some(1), &[4, 3]), Some(1), "its parent");
        assert_eq!(t.skeleton(Some(0), &[4, 3]), Some(0), "its grandparent");
        assert_eq!(t.skeleton(Some(4), &[4, 3]), Some(3), "a child");
        assert_eq!(t.skeleton(Some(2), &[4, 3]), Some(3), "a sibling branch");
        assert_eq!(t.skeleton(Some(6), &[4, 3]), Some(3), "another tree");
        assert_eq!(t.skeleton(None, &[4, 3]), Some(3), "none held");
        assert_eq!(t.skeleton(Some(0), &[4, 6]), None, "joints in two trees");
    }

    /// The same at the node cap: a chain of `MAX_NODES` nodes, where a set's
    /// closest common root is its shallowest node, and the same nodes as two
    /// chains forking at node 0, where a pair across the fork meets at 0.
    /// Answers only, no timing.
    #[test]
    fn closest_common_roots_on_a_chain_at_the_node_cap() {
        let n = super::super::scene::MAX_NODES;
        let half = n / 2;
        let chain: Vec<Option<usize>> = (0..n).map(|i| i.checked_sub(1)).collect();
        let fork: Vec<Option<usize>> = (0..n).map(|i| if i == half { Some(0) } else { i.checked_sub(1) }).collect();
        let (c, f) = (Ancestry::new(&chain), Ancestry::new(&fork));
        // Which chain of the fork a node is on: 0 for the fork itself.
        let side = |i: usize| if i == 0 { 0 } else if i < half { 1 } else { 2 };
        let across = |i: usize, j: usize| if side(i) == side(j) && side(i) != 0 { i.min(j) } else { 0 };
        for i in (0..n).step_by(97) {
            assert_eq!(c.closest_common_root(&[i]), Some(i));
            assert_eq!(f.closest_common_root(&[i]), Some(i));
            for j in [0, 1, half - 1, half, half + 1, n - 1, (i * 31 + 7) % n] {
                assert_eq!(c.closest_common_root(&[i, j]), Some(i.min(j)), "chain {i} {j}");
                assert_eq!(c.closest_common_root(&[j, i]), Some(i.min(j)), "chain {j} {i}");
                assert_eq!(f.closest_common_root(&[i, j]), Some(across(i, j)), "fork {i} {j}");
                assert_eq!(f.closest_common_root(&[j, i]), Some(across(i, j)), "fork {j} {i}");
            }
        }
        // Every node at once, listed root first and deepest first.
        let all: Vec<usize> = (0..n).collect();
        let deepest_first: Vec<usize> = (0..n).rev().collect();
        assert_eq!(c.closest_common_root(&all), Some(0));
        assert_eq!(c.closest_common_root(&deepest_first), Some(0));
        let second: Vec<usize> = (half..n).rev().collect();
        assert_eq!(f.closest_common_root(&second), Some(half));
        assert_eq!(f.closest_common_root(&deepest_first), Some(0));
    }

    // ── header and chunk errors ─────────────────────────────────────────────

    #[test]
    fn rejects_a_short_buffer_as_not_this_format() {
        let e = read(b"glTF").unwrap_err();
        assert!(matches!(e, MeshError::NotThisFormat { expected: MeshFormat::Glb, .. }), "{e:?}");
    }

    #[test]
    fn rejects_foreign_magic_as_not_this_format() {
        let mut glb = triangle_glb();
        glb[0..4].copy_from_slice(b"OBJ ");
        assert!(matches!(read(&glb), Err(MeshError::NotThisFormat { .. })));
    }

    #[test]
    fn rejects_gltf_1_as_not_this_format() {
        let mut glb = triangle_glb();
        glb[4..8].copy_from_slice(&1u32.to_le_bytes());
        let e = read(&glb).unwrap_err();
        match e {
            MeshError::NotThisFormat { detail, .. } => assert!(detail.contains("version 1"), "{detail}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_a_truncated_file_by_its_declared_length() {
        let glb = triangle_glb();
        let d = unwrap_malformed(read(&glb[..glb.len() - 8]).unwrap_err());
        assert!(d.contains("truncated"), "{d}");
    }

    #[test]
    fn rejects_a_declared_length_below_the_header() {
        let mut glb = triangle_glb();
        glb[8..12].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(read(&glb), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn rejects_a_hostile_chunk_length() {
        let mut glb = triangle_glb();
        // The JSON chunk claims the whole 32-bit address space.
        glb[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        let d = unwrap_malformed(read(&glb).unwrap_err());
        assert!(d.contains("chunk at offset 12"), "{d}");
    }

    #[test]
    fn rejects_a_glb_with_no_json_chunk() {
        let mut glb = triangle_glb();
        // Retype the JSON chunk as an unknown type: it is then ignored, and
        // nothing describes the file.
        glb[16..20].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let d = unwrap_malformed(read(&glb).unwrap_err());
        assert!(d.contains("no JSON chunk"), "{d}");
    }

    #[test]
    fn ignores_unknown_chunks_after_the_known_ones() {
        let mut glb = triangle_glb();
        glb.extend_from_slice(&4u32.to_le_bytes());
        glb.extend_from_slice(&0x0BAD_F00Du32.to_le_bytes());
        glb.extend_from_slice(&[1, 2, 3, 4]);
        let total = glb.len() as u32;
        glb[8..12].copy_from_slice(&total.to_le_bytes());
        assert_eq!(read(&glb).unwrap().triangle_count(), 1);
    }

    #[test]
    fn ignores_trailing_bytes_past_the_declared_length() {
        let mut glb = triangle_glb();
        glb.extend_from_slice(b"tar padding, not ours");
        assert_eq!(read(&glb).unwrap().triangle_count(), 1);
    }

    #[test]
    fn rejects_unparseable_json() {
        let mut glb = triangle_glb();
        glb[20] = b'%';
        let d = unwrap_malformed(read(&glb).unwrap_err());
        assert!(d.contains("does not parse"), "{d}");
    }

    #[test]
    fn tolerates_a_json_chunk_padded_with_nuls() {
        let (doc, bin) = triangle_doc();
        // Rebuild by hand so the padding is NUL rather than the spec's space.
        let mut json = serde_json::to_vec(&doc).unwrap();
        while json.len() % 4 != 0 {
            json.push(0);
        }
        let mut b = bin.clone();
        pad_to_4(&mut b, 0);
        let total = 12 + 8 + json.len() + 8 + b.len();
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&(total as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(b.len() as u32).to_le_bytes());
        glb.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        glb.extend_from_slice(&b);
        assert_eq!(read(&glb).unwrap().triangle_count(), 1);
    }

    // ── geometry ────────────────────────────────────────────────────────────

    #[test]
    fn reads_a_minimal_indexed_triangle() {
        let mesh = read(&triangle_glb()).unwrap();
        assert_eq!(mesh.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.normals, None);
        assert_eq!(mesh.uvs, None);
    }

    #[test]
    fn synthesises_indices_for_a_non_indexed_primitive() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]
            .as_object_mut()
            .unwrap()
            .remove("indices");
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.indices, vec![0, 1, 2]);
    }

    #[test]
    fn rejects_a_non_indexed_primitive_whose_vertex_count_is_not_a_multiple_of_three() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0].as_object_mut().unwrap().remove("indices");
        doc["accessors"][0]["count"] = Value::from(2);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("not a multiple of 3"), "{d}");
    }

    #[test]
    fn reads_unsigned_short_and_unsigned_byte_indices() {
        for (comp, bytes) in [(5123u64, u16s(&[0, 1, 2])), (5121, vec![0u8, 1, 2])] {
            let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            let start = bin.len();
            let len = bytes.len();
            bin.extend_from_slice(&bytes);
            let (mut doc, _) = triangle_doc();
            doc["accessors"][1]["componentType"] = Value::from(comp);
            doc["bufferViews"][1]["byteOffset"] = Value::from(start);
            doc["bufferViews"][1]["byteLength"] = Value::from(len);
            doc["buffers"][0]["byteLength"] = Value::from(bin.len());
            assert_eq!(read(&pack(doc, Some(&bin))).unwrap().indices, vec![0, 1, 2], "comp {comp}");
        }
    }

    #[test]
    fn rejects_a_signed_index_component_type() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][1]["componentType"] = Value::from(5122); // SHORT
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("UNSIGNED_BYTE"), "{d}");
    }

    #[test]
    fn rejects_an_index_past_the_vertex_count() {
        let (doc, mut bin) = triangle_doc();
        let start = bin.len() - 12;
        bin[start..].copy_from_slice(&u32s(&[0, 1, 7]));
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("out of range for 3 vertices"), "{d}");
    }

    #[test]
    fn rejects_an_index_count_that_is_not_a_multiple_of_three() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][1]["count"] = Value::from(2);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("not a multiple of 3"), "{d}");
    }

    #[test]
    fn rejects_a_hostile_accessor_count() {
        let (mut doc, bin) = triangle_doc();
        // 4 billion positions behind 36 bytes of buffer: must be an error, not
        // an allocation and not a panic.
        doc["accessors"][0]["count"] = Value::from(u32::MAX);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("reads past the end"), "{d}");
    }

    #[test]
    fn rejects_an_accessor_pointing_past_its_buffer() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["byteOffset"] = Value::from(1000);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("reads past the end"), "{d}");
    }

    #[test]
    fn rejects_out_of_range_indices_into_the_json_arrays() {
        for (path, value, needle) in [
            ("/meshes/0/primitives/0/attributes/POSITION", 9, "accessor index 9"),
            ("/accessors/0/bufferView", 9, "bufferView index 9"),
            ("/nodes/0/mesh", 9, "mesh index 9"),
        ] {
            let (mut doc, bin) = triangle_doc();
            *doc.pointer_mut(path).unwrap() = Value::from(value);
            let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
            assert!(d.contains(needle), "{path}: {d}");
        }
    }

    #[test]
    fn honours_buffer_view_stride_for_interleaved_attributes() {
        // POSITION and NORMAL interleaved in one 24-byte-stride view.
        let mut bin = Vec::new();
        for (p, n) in [
            ([0.0f32, 0.0, 0.0], [0.0f32, 0.0, 1.0]),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ] {
            bin.extend_from_slice(&f32s(&p));
            bin.extend_from_slice(&f32s(&n));
        }
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}
            ]}],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": 12, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start, "byteStride": 24},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        assert_eq!(
            mesh.normals,
            Some(vec![[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]])
        );
    }

    #[test]
    fn rejects_a_stride_smaller_than_the_element() {
        let (mut doc, bin) = triangle_doc();
        doc["bufferViews"][0]["byteStride"] = Value::from(8);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("byteStride 8"), "{d}");
    }

    #[test]
    fn merges_every_primitive_and_offsets_the_indices() {
        // Two primitives (as a material split would produce) in one mesh.
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        bin.extend_from_slice(&f32s(&[2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 2.0, 1.0, 0.0]));
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0}, "indices": 2, "material": 0},
                {"attributes": {"POSITION": 1}, "indices": 2, "material": 1}
            ]}],
            "materials": [
                {"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0, 0.0, 1.0]}},
                {"pbrMetallicRoughness": {"baseColorFactor": [0.0, 1.0, 0.0, 1.0]}}
            ],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": 36, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.positions.len(), 6);
        assert_eq!(mesh.indices, vec![0, 1, 2, 3, 4, 5], "second primitive must be offset");
        assert_eq!(mesh.base_color, Some([1.0, 0.0, 0.0, 1.0]), "first material wins");
    }

    /// The flat mesh is triangles alone: a line primitive (one segment, from
    /// the first two of the triangle's indices) is read into the scene and
    /// left out of the flat mesh.
    #[test]
    fn skips_primitives_that_are_not_triangles() {
        let (mut doc, bin) = triangle_doc();
        let prim = doc["meshes"][0]["primitives"][0].clone();
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 1, "componentType": 5125, "count": 2, "type": "SCALAR"}));
        let mut line = prim.clone();
        line["mode"] = Value::from(1);
        line["indices"] = Value::from(2);
        doc["meshes"][0]["primitives"] = Value::Array(vec![line, prim]);
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.triangle_count(), 1, "the line primitive contributes nothing");
    }

    #[test]
    fn reports_empty_when_nothing_is_a_triangle() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]["mode"] = Value::from(0); // POINTS
        assert_eq!(read(&pack(doc, Some(&bin))), Err(MeshError::Empty));
    }

    #[test]
    fn rejects_a_non_numeric_mode() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]["mode"] = Value::from("triangles");
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("non-numeric mode"), "{d}");
    }

    /// glTF 2.0 §3.7.2.1: "When positions are not specified, client
    /// implementations SHOULD skip primitive's rendering" — skipped with a
    /// warning, not the whole file refused (2026-10-12 review, finding 27).
    #[test]
    fn a_primitive_without_positions_is_skipped_with_a_warning() {
        let (mut doc, bin) = triangle_doc();
        let mut bare = doc["meshes"][0]["primitives"][0].clone();
        bare["attributes"] = serde_json::json!({"NORMAL": 0});
        doc["meshes"][0]["primitives"].as_array_mut().unwrap().push(bare);
        let r = read_scene(&pack(doc.clone(), Some(&bin)), &NoSidecars).unwrap();
        assert_eq!(r.scene.triangle_count(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("no POSITION")), "{:?}", r.warnings);
        // A file with nothing but such primitives has nothing to draw.
        doc["meshes"][0]["primitives"][0]["attributes"] = serde_json::json!({});
        doc["meshes"][0]["primitives"].as_array_mut().unwrap().truncate(1);
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Empty)));
    }

    #[test]
    fn rejects_an_attribute_count_that_disagrees_with_positions() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 0, "componentType": 5126, "count": 2, "type": "VEC3"
        }));
        doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = Value::from(2);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("2 normals for 3 positions"), "{d}");
    }

    // ── transforms ──────────────────────────────────────────────────────────

    #[test]
    fn applies_an_accumulated_node_transform() {
        let (mut doc, bin) = triangle_doc();
        // Root translates by +10 on x; the child scales by 2. The mesh hangs off
        // the child, so both must apply, in that order.
        doc["scenes"] = serde_json::json!([{"nodes": [0]}]);
        doc["nodes"] = serde_json::json!([
            {"translation": [10.0, 0.0, 0.0], "children": [1]},
            {"mesh": 0, "scale": [2.0, 2.0, 2.0]}
        ]);
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.positions, vec![[10.0, 0.0, 0.0], [12.0, 0.0, 0.0], [10.0, 2.0, 0.0]]);
    }

    #[test]
    fn applies_a_root_rotation_to_positions_and_normals() {
        let (mut doc, bin) = triangle_doc();
        // The -90° about X that every Y-up→Z-up exporter bakes in.
        let s = (std::f64::consts::FRAC_PI_4).sin();
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3"
        }));
        doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = Value::from(2);
        let mut bin = bin;
        let normal_start = bin.len();
        bin.extend_from_slice(&f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": normal_start, "byteLength": 36
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["nodes"] = serde_json::json!([{"mesh": 0, "rotation": [-s, 0.0, 0.0, s]}]);

        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        // (0,1,0) rotates to (0,0,-1) under -90° about X.
        let p = mesh.positions[2];
        assert!(p[1].abs() < 1e-6 && (p[2] + 1.0).abs() < 1e-6, "{p:?}");
        // +Z normals rotate to +Y.
        let n = mesh.normals.unwrap()[0];
        assert!((n[1] - 1.0).abs() < 1e-6 && n[2].abs() < 1e-6, "{n:?}");
    }

    #[test]
    fn applies_a_column_major_matrix() {
        let (mut doc, bin) = triangle_doc();
        // Translate +5 on y: the translation lives in the last column, which is
        // elements 12..15 of a column-major array.
        doc["nodes"] = serde_json::json!([{
            "mesh": 0,
            "matrix": [1.0, 0.0, 0.0, 0.0,  0.0, 1.0, 0.0, 0.0,  0.0, 0.0, 1.0, 0.0,  0.0, 5.0, 0.0, 1.0]
        }]);
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.positions[0], [0.0, 5.0, 0.0]);
    }

    #[test]
    fn flips_winding_under_a_mirroring_transform() {
        let (mut doc, bin) = triangle_doc();
        doc["nodes"] = serde_json::json!([{"mesh": 0, "scale": [-1.0, 1.0, 1.0]}]);
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.indices, vec![0, 2, 1], "a mirror must reverse the winding");
    }

    #[test]
    fn non_uniform_scale_keeps_normals_perpendicular() {
        // A 45° normal under a 10x x-scale: the inverse-transpose leans it
        // toward +y, the naive upper-3x3 would lean it toward +x.
        let (mut doc, mut bin) = triangle_doc();
        let normal_start = bin.len();
        let d = 1.0f32 / 2.0f32.sqrt();
        bin.extend_from_slice(&f32s(&[d, d, 0.0, d, d, 0.0, d, d, 0.0]));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3"
        }));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": normal_start, "byteLength": 36
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = Value::from(2);
        doc["nodes"] = serde_json::json!([{"mesh": 0, "scale": [10.0, 1.0, 1.0]}]);
        let n = read(&pack(doc, Some(&bin))).unwrap().normals.unwrap()[0];
        assert!(n[1] > n[0], "inverse-transpose must tilt the normal to +y, got {n:?}");
    }

    #[test]
    fn rejects_a_malformed_node_transform() {
        for (key, value, needle) in [
            ("matrix", serde_json::json!([1.0, 2.0]), "expected 16"),
            ("rotation", serde_json::json!([0.0, 0.0, 0.0, 0.0]), "zero-length"),
            ("translation", serde_json::json!([1.0, 2.0]), "not a 3-array"),
            ("scale", serde_json::json!("big"), "not a 3-array"),
        ] {
            let (mut doc, bin) = triangle_doc();
            doc["nodes"][0][key] = value;
            let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
            assert!(d.contains(needle), "{key}: {d}");
        }
    }

    #[test]
    fn rejects_a_node_graph_that_is_not_a_tree() {
        let (mut doc, bin) = triangle_doc();
        doc["scenes"] = serde_json::json!([{"nodes": [0]}]);
        doc["nodes"] = serde_json::json!([{"children": [1]}, {"children": [1], "mesh": 0}]);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("not a tree"), "{d}");
    }

    #[test]
    fn falls_back_to_every_mesh_when_the_file_has_no_scene() {
        let (mut doc, bin) = triangle_doc();
        let obj = doc.as_object_mut().unwrap();
        obj.remove("scene");
        obj.remove("scenes");
        obj.remove("nodes");
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().triangle_count(), 1);
    }

    #[test]
    fn defaults_to_scene_zero_when_scene_is_absent() {
        let (mut doc, bin) = triangle_doc();
        doc.as_object_mut().unwrap().remove("scene");
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().triangle_count(), 1);
    }

    #[test]
    fn rejects_an_out_of_range_scene() {
        let (mut doc, bin) = triangle_doc();
        doc["scene"] = Value::from(3);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("scene index 3"), "{d}");
    }

    // ── attributes and materials ────────────────────────────────────────────

    #[test]
    fn decodes_normalised_short_uvs() {
        // Core glTF allows UNSIGNED_SHORT normalised texture coordinates; they
        // used to be refused only because the flat IR had no use for decoding
        // anything but floats. 65535 is exactly 1.0.
        let (mut doc, mut bin) = triangle_doc();
        let uv_start = bin.len();
        bin.extend_from_slice(&u16s(&[0, 0, 65535, 0, 0, 65535]));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 2, "componentType": 5123, "count": 3, "type": "VEC2", "normalized": true
        }));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": uv_start, "byteLength": 12
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = Value::from(2);
        let uvs = read(&pack(doc, Some(&bin))).unwrap().uvs.unwrap();
        assert_eq!(uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
    }

    #[test]
    fn refuses_unnormalised_integer_uvs_without_quantization() {
        // The same bytes NOT normalised are KHR_mesh_quantization's encoding;
        // without the extension declared they are not valid glTF, and reading
        // them as 0..65535 would wrap every texture thousands of times.
        let (mut doc, mut bin) = triangle_doc();
        let uv_start = bin.len();
        bin.extend_from_slice(&u16s(&[0, 0, 65535, 0, 0, 65535]));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 2, "componentType": 5123, "count": 3, "type": "VEC2"
        }));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": uv_start, "byteLength": 12
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = Value::from(2);
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("TEXCOORD_0") && d.contains("5123"), "{d}");
    }

    #[test]
    fn rejects_quantised_positions_as_unsupported() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["componentType"] = Value::from(5122);
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("POSITION"), "{d}");
    }

    #[test]
    fn rejects_a_position_accessor_that_is_not_vec3() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["type"] = Value::from("VEC2");
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("expected VEC3"), "{d}");
    }

    #[test]
    fn rejects_an_unknown_accessor_type() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["type"] = Value::from("VEC7");
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn a_matrix_accessor_used_as_positions_is_malformed() {
        // FLOAT MAT4 is now a supported accessor (inverse bind matrices), so
        // naming one as POSITION is simply the wrong type — and, with 36 bytes
        // behind it, too short besides. Either way it must not read.
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["type"] = Value::from("MAT4");
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn refuses_non_float_matrix_accessors() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["type"] = Value::from("MAT3");
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn reads_base_color_from_the_first_primitive_that_has_one() {
        let (mut doc, bin) = triangle_doc();
        doc["materials"] = serde_json::json!([
            {"pbrMetallicRoughness": {"baseColorFactor": [0.5, 0.25, 0.125, 0.75]}}
        ]);
        doc["meshes"][0]["primitives"][0]["material"] = Value::from(0);
        assert_eq!(
            read(&pack(doc, Some(&bin))).unwrap().base_color,
            Some([0.5, 0.25, 0.125, 0.75])
        );
    }

    #[test]
    fn leaves_base_color_unset_when_the_material_has_no_factor() {
        let (mut doc, bin) = triangle_doc();
        doc["materials"] = serde_json::json!([{"pbrMetallicRoughness": {"metallicFactor": 0.0}}]);
        doc["meshes"][0]["primitives"][0]["material"] = Value::from(0);
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().base_color, None);
    }

    #[test]
    fn rejects_a_malformed_base_color_factor() {
        let (mut doc, bin) = triangle_doc();
        doc["materials"] =
            serde_json::json!([{"pbrMetallicRoughness": {"baseColorFactor": [1.0, 0.0]}}]);
        doc["meshes"][0]["primitives"][0]["material"] = Value::from(0);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("baseColorFactor"), "{d}");
    }

    #[test]
    fn takes_the_name_from_the_first_merged_mesh() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["name"] = Value::from("dragon");
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().name, Some("dragon".into()));
    }

    #[test]
    fn derives_normals_only_for_the_primitives_that_lack_them() {
        // Two primitives, one with normals and one without: the file's own
        // normals must survive untouched and the gap must be filled, because the
        // IR cannot express "normals for half the vertices".
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let n_start = bin.len();
        bin.extend_from_slice(&f32s(&[1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0]));
        let p2_start = bin.len();
        bin.extend_from_slice(&f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]));
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 3},
                {"attributes": {"POSITION": 2}, "indices": 3}
            ]}],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": n_start, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": p2_start, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let normals = read(&pack(doc, Some(&bin))).unwrap().normals.unwrap();
        assert_eq!(&normals[..3], &[[1.0, 0.0, 0.0]; 3], "shipped normals must survive");
        // The second triangle lies in z=0 wound CCW, so its facet normal is +z.
        assert_eq!(&normals[3..], &[[0.0, 0.0, 1.0]; 3]);
    }

    #[test]
    fn fills_missing_uvs_with_zero_rather_than_dropping_the_real_ones() {
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        let uv_start = bin.len();
        bin.extend_from_slice(&f32s(&[0.25, 0.5, 0.25, 0.5, 0.25, 0.5]));
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0, "TEXCOORD_0": 1}, "indices": 2},
                {"attributes": {"POSITION": 0}, "indices": 2}
            ]}],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": uv_start, "componentType": 5126, "count": 3, "type": "VEC2"},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let uvs = read(&pack(doc, Some(&bin))).unwrap().uvs.unwrap();
        assert_eq!(&uvs[..3], &[[0.25, 0.5]; 3]);
        assert_eq!(&uvs[3..], &[[0.0, 0.0]; 3]);
    }

    // ── buffers ─────────────────────────────────────────────────────────────

    #[test]
    fn reads_a_base64_data_uri_buffer() {
        let (mut doc, bin) = triangle_doc();
        doc["buffers"][0]["uri"] = Value::from(format!("data:application/octet-stream;base64,{}", b64(&bin)));
        // No BIN chunk at all: the geometry lives entirely in the JSON.
        assert_eq!(read(&pack(doc, None)).unwrap().triangle_count(), 1);
    }

    #[test]
    fn rejects_an_external_buffer_uri_as_unsupported() {
        let (mut doc, bin) = triangle_doc();
        doc["buffers"][0]["uri"] = Value::from("model.bin");
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("model.bin"), "{d}");
    }

    #[test]
    fn rejects_a_non_base64_data_uri_as_unsupported() {
        let (mut doc, bin) = triangle_doc();
        doc["buffers"][0]["uri"] = Value::from("data:application/octet-stream,%00%01");
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn rejects_invalid_base64() {
        let (mut doc, bin) = triangle_doc();
        doc["buffers"][0]["uri"] = Value::from("data:application/octet-stream;base64,!!!!");
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("base64"), "{d}");
    }

    #[test]
    fn rejects_a_uri_less_buffer_with_no_bin_chunk() {
        let (doc, _) = triangle_doc();
        let d = unwrap_malformed(read(&pack(doc, None)).unwrap_err());
        assert!(d.contains("no BIN chunk"), "{d}");
    }

    #[test]
    fn ignores_an_external_buffer_nothing_reads() {
        // A second buffer for the textures we never touch must not condemn the
        // file — only a buffer we actually read can.
        let (mut doc, bin) = triangle_doc();
        doc["buffers"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"uri": "textures.bin", "byteLength": 4}));
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().triangle_count(), 1);
    }

    #[test]
    fn base64_decoder_matches_known_vectors() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");
        assert_eq!(decode_base64("Zm9v\nYmFy").unwrap(), b"foobar");
        assert_eq!(decode_base64("Zm9vYmE=").unwrap(), b"fooba");
        assert!(decode_base64("Zg=A").is_none(), "payload after padding");
        assert!(decode_base64("Zm9*").is_none(), "illegal character");
        assert!(decode_base64("Z").is_none(), "a lone sextet is not a byte");
    }

    // ── extensions ──────────────────────────────────────────────────────────

    /// A required extension this reader doesn't implement is refused, naming
    /// it. It was `rejects_a_required_compression_extension`, on
    /// KHR_draco_mesh_compression, which now decodes; like matrix_tests.rs
    /// (Ruling B17), it names `EXT_litegen_mock_undecodable`, which nothing
    /// will ever implement.
    #[test]
    fn rejects_a_required_extension_it_does_not_implement() {
        let (mut doc, bin) = triangle_doc();
        doc["extensionsRequired"] = serde_json::json!(["EXT_litegen_mock_undecodable"]);
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("EXT_litegen_mock_undecodable"), "{d}");
    }

    #[test]
    fn ignores_a_required_material_only_extension() {
        let (mut doc, bin) = triangle_doc();
        doc["extensionsRequired"] = serde_json::json!(["KHR_materials_unlit"]);
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().triangle_count(), 1);
    }

    /// KHR_draco_mesh_compression only *used*: the primitive's accessors also
    /// carry plain data, the fallback for a reader without a Draco decoder.
    /// This reader decodes the stream and never reads that fallback, as the
    /// extension's README requires of a loader that supports it (:132: "you
    /// must ignore the bufferView and byteOffset of the accessor"; Ruling
    /// B27). So Box reads from its stream whether its fallback (every
    /// position at 10) is supplied or missing, and nothing warns. This was
    /// `a_draco_primitive_falls_back_to_its_uncompressed_data_unless_draco_is_required`,
    /// which pinned the fallback read (2026-10-12 review, finding 27).
    #[test]
    fn a_draco_primitive_decodes_its_stream_even_when_draco_is_only_used() {
        let mut doc = box_draco_doc();
        doc.as_object_mut().unwrap().remove("extensionsRequired");
        let mut fallback = f32s(&[10.0; 72]);
        fallback.extend(f32s(&[0.0, 0.0, 1.0].repeat(24)));
        fallback.extend(u16s(&(0..36).map(|i| i % 24).collect::<Vec<u16>>()));
        doc["buffers"].as_array_mut().unwrap().push(serde_json::json!({"uri": "fallback.bin", "byteLength": fallback.len()}));
        // Box's accessors: 0 the indices, 1 NORMAL, 2 POSITION.
        for (accessor, offset, length) in [(2usize, 0usize, 288usize), (1, 288, 288), (0, 576, 72)] {
            let view = doc["bufferViews"].as_array().unwrap().len();
            doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({"buffer": 1, "byteOffset": offset, "byteLength": length}));
            doc["accessors"][accessor]["bufferView"] = serde_json::json!(view);
        }
        for supplied in [true, false] {
            let mut files = vec![PackageFile { path: "Box.bin".into(), bytes: draco_sample("Box").draco[1].to_vec() }];
            if supplied {
                files.push(PackageFile { path: "fallback.bin".into(), bytes: fallback.clone() });
            }
            let r = read_document(doc.clone(), None, &package::MemorySidecars { files: &files })
                .unwrap_or_else(|e| panic!("fallback supplied: {supplied}: {e}"));
            let p = &r.scene.meshes[0].primitives[0];
            assert_eq!((p.vertex_count(), p.triangle_count()), (24, 12), "fallback supplied: {supplied}");
            assert!(p.positions.iter().flatten().all(|c| c.abs() == 0.5), "fallback supplied: {supplied}: the stream's corners, not the fallback's");
            assert!(r.warnings.is_empty(), "fallback supplied: {supplied}: {:?}", r.warnings);
        }
    }

    #[test]
    fn rejects_a_sparse_accessor_with_no_substitution_list() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["sparse"] = serde_json::json!({"count": 1});
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("sparse"), "{d}");
    }

    /// The triangle with a sparse patch moving vertex 2 to (0, 5, 0).
    fn sparse_doc(indices: &[u32], count: usize) -> (Value, Vec<u8>) {
        let (mut doc, mut bin) = triangle_doc();
        let idx_start = bin.len();
        bin.extend_from_slice(&u32s(indices));
        let val_start = bin.len();
        for _ in indices {
            bin.extend_from_slice(&f32s(&[0.0, 5.0, 0.0]));
        }
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": idx_start, "byteLength": indices.len() * 4
        }));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": val_start, "byteLength": indices.len() * 12
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["accessors"][0]["sparse"] = serde_json::json!({
            "count": count,
            "indices": {"bufferView": 2, "componentType": 5125},
            "values": {"bufferView": 3}
        });
        (doc, bin)
    }

    #[test]
    fn applies_a_sparse_substitution_over_the_base_view() {
        let (doc, bin) = sparse_doc(&[2], 1);
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 5.0, 0.0]]);
    }

    #[test]
    fn rejects_sparse_indices_out_of_range_or_out_of_order() {
        for (indices, count) in [(vec![3u32], 1), (vec![1, 1], 2), (vec![2, 0], 2)] {
            let (doc, bin) = sparse_doc(&indices, count);
            let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
            assert!(d.contains("sparse"), "{indices:?}: {d}");
        }
    }

    #[test]
    fn rejects_a_sparse_count_larger_than_the_accessor() {
        let (doc, bin) = sparse_doc(&[0, 1, 2, 3], 4);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("sparse count"), "{d}");
    }

    /// glTF 2.0 §3.6.2.1: an accessor with no bufferView (and no sparse
    /// patch) reads as zeros — Blender writes an empty shape key that way.
    /// The zeros are charged to the decode budget, so `count` alone cannot
    /// become an allocation (2026-10-12 review, finding 27: refusing it was a
    /// regression).
    #[test]
    fn an_accessor_without_a_buffer_view_reads_as_zeros() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]["targets"] = serde_json::json!([{"POSITION": 2}]);
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [0.0, 0.0, 0.0]
        }));
        let s = read_scene(&pack(doc, Some(&bin)), &NoSidecars).unwrap().scene;
        let target = &s.meshes[0].primitives[0].targets[0];
        assert_eq!(target.positions, vec![[0.0f32; 3]; 3]);
    }

    // ── allocation ceilings ─────────────────────────────────────────────────

    #[test]
    fn a_view_less_accessor_cannot_ask_for_an_unbounded_allocation() {
        // 244 bytes of input, 20 million elements claimed, no BIN chunk to hold
        // them: this returned Ok having allocated 240 MB, and at 4 billion it
        // asked for 48 GB and the allocator aborted the process.
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{"componentType": 5126, "count": 20_000_001u64, "type": "VEC3"}],
            "buffers": []
        });
        let glb = pack(doc, None);
        assert!(glb.len() < 1024, "the input has to be tiny for the point to hold");
        let started = std::time::Instant::now();
        assert!(read(&glb).is_err());
        assert!(started.elapsed().as_millis() < 250, "it allocated before refusing");

        // 4 billion is the one that aborted rather than returning.
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{"componentType": 5126, "count": 4_000_000_000u64, "type": "VEC3"}],
            "buffers": []
        });
        assert!(read(&pack(doc, None)).is_err());
    }

    #[test]
    fn a_count_larger_than_its_buffer_view_is_refused_before_it_is_read() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["count"] = Value::from(500_000_000u64);
        let started = std::time::Instant::now();
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("reads past the end"), "{d}");
        assert!(started.elapsed().as_millis() < 250, "it allocated before refusing");
    }

    #[test]
    fn instancing_cannot_amplify_one_accessor_past_the_merge_ceiling() {
        // The second vector: geometry is re-read per node instance, so a small
        // file with many nodes pointing at one mesh merges to any size it likes.
        let vcount = 99_999usize; // sequential indices, so a multiple of 3
        let bin: Vec<u8> = f32s(&vec![0.5f32; vcount * 3]);
        let instances = 45;
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": (0..instances).collect::<Vec<_>>()}],
            "nodes": (0..instances).map(|_| serde_json::json!({"mesh": 0})).collect::<Vec<_>>(),
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": vcount, "type": "VEC3"}
            ],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": bin.len()}],
            "buffers": [{"byteLength": bin.len()}]
        });
        let glb = pack(doc, Some(&bin));
        assert!(glb.len() < 2 * 1024 * 1024, "{} bytes in", glb.len());
        let d = unwrap_unsupported(read(&glb).unwrap_err());
        assert!(d.contains(&MAX_MERGED_VERTICES.to_string()), "{d}");
    }

    // ── bufferView bounds ───────────────────────────────────────────────────

    #[test]
    fn an_accessor_may_not_read_past_its_own_buffer_view() {
        // One vertex declared, three read: the other 24 bytes were the index
        // array next door, handed back as geometry with no complaint.
        let (mut doc, bin) = triangle_doc();
        doc["bufferViews"][0]["byteLength"] = Value::from(12);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("reads past the end") && d.contains("bufferView 0"), "{d}");
    }

    #[test]
    fn an_interleaved_view_is_bounded_by_its_own_length_not_the_buffers() {
        // The last element of a strided view ends at byteOffset + (count-1) *
        // stride + size, and that is the number the view has to cover.
        let (mut doc, bin) = triangle_doc();
        doc["bufferViews"][0]["byteStride"] = Value::from(12);
        doc["bufferViews"][0]["byteLength"] = Value::from(35); // one byte short
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("reads past the end"), "{d}");
    }

    #[test]
    fn rejects_a_buffer_view_that_runs_past_its_buffer() {
        let (mut doc, bin) = triangle_doc();
        doc["bufferViews"][0]["byteLength"] = Value::from(4096);
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("bufferView 0 spans"), "{d}");
    }

    #[test]
    fn rejects_a_buffer_view_with_no_byte_length() {
        let (mut doc, bin) = triangle_doc();
        doc["bufferViews"][0].as_object_mut().unwrap().remove("byteLength");
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("no byteLength"), "{d}");
    }

    // ── present-but-unreadable fields ───────────────────────────────────────

    #[test]
    fn a_float_valued_index_is_an_error_rather_than_an_absent_field() {
        // serde_json's as_u64 rejects anything parsed as f64, so `1.0` out of an
        // exporter that round-tripped its numbers through a float used to look
        // exactly like an omitted field and take the default branch.
        for (path, needle) in [
            ("/meshes/0/primitives/0/indices", "indices"),
            ("/meshes/0/primitives/0/material", "material"),
            ("/meshes/0/primitives/0/attributes/POSITION", "POSITION"),
            ("/nodes/0/mesh", "mesh"),
            ("/scene", "scene"),
            ("/accessors/0/byteOffset", "byteOffset"),
            ("/bufferViews/0/byteOffset", "byteOffset"),
            ("/bufferViews/0/byteStride", "byteStride"),
            ("/bufferViews/0/byteLength", "byteLength"),
            ("/accessors/0/bufferView", "bufferView"),
        ] {
            let (mut doc, bin) = triangle_doc();
            // Several of these are absent from the fixture (`material`,
            // `byteStride`, the byteOffsets); the case under test is what a
            // PRESENT one that will not read as an index does.
            match doc.pointer_mut(path) {
                Some(v) => *v = Value::from(1.0),
                None => {
                    let (parent, key) = path.rsplit_once('/').unwrap();
                    doc.pointer_mut(parent).unwrap().as_object_mut().unwrap()
                        .insert(key.to_string(), Value::from(1.0));
                }
            }
            let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
            assert!(d.contains(needle) && d.contains("not an index"), "{path}: {d}");
        }
    }

    #[test]
    fn a_float_indices_no_longer_reverses_the_winding() {
        // The worst of the set: the index accessor was ignored and a sequential
        // list synthesised, so a file saying [2,1,0] read back as [0,1,2] and
        // the model rendered inside out.
        let (doc, mut bin) = triangle_doc();
        let start = bin.len() - 12;
        bin[start..].copy_from_slice(&u32s(&[2, 1, 0]));
        assert_eq!(read(&pack(doc.clone(), Some(&bin))).unwrap().indices, vec![2, 1, 0]);

        let mut doc = doc;
        doc["meshes"][0]["primitives"][0]["indices"] = Value::from(1.0);
        assert!(read(&pack(doc, Some(&bin))).is_err(), "winding silently reversed");
    }

    #[test]
    fn a_float_byte_stride_no_longer_reads_normals_as_positions() {
        // POSITION and NORMAL interleaved at stride 24: read as packed, every
        // other vertex is a normal.
        let mut bin = Vec::new();
        for (p, n) in [
            ([0.0f32, 0.0, 0.0], [0.0f32, 0.0, 1.0]),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ] {
            bin.extend_from_slice(&f32s(&p));
            bin.extend_from_slice(&f32s(&n));
        }
        let index_start = bin.len();
        bin.extend_from_slice(&u32s(&[0, 1, 2]));
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}
            ]}],
            "accessors": [
                {"bufferView": 0, "byteOffset": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 0, "byteOffset": 12, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 1, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": index_start, "byteStride": 24.0},
                {"buffer": 0, "byteOffset": index_start, "byteLength": 12}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("byteStride"), "{d}");
    }

    #[test]
    fn a_null_valued_index_is_an_error_too() {
        let (mut doc, bin) = triangle_doc();
        doc["nodes"][0]["mesh"] = Value::Null;
        assert!(matches!(read(&pack(doc, Some(&bin))), Err(MeshError::Malformed(_))));
    }

    // ── extension guards ────────────────────────────────────────────────────

    #[test]
    fn rejects_a_non_array_extensions_required() {
        // A string fell through to Ok, disabling the guard completely. Draco had
        // a per-primitive backstop; meshopt had none.
        for value in [
            serde_json::json!("EXT_meshopt_compression"),
            serde_json::json!("KHR_draco_mesh_compression"),
            serde_json::json!({"0": "EXT_meshopt_compression"}),
        ] {
            let (mut doc, bin) = triangle_doc();
            doc["extensionsRequired"] = value.clone();
            let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
            assert!(d.contains("extensionsRequired"), "{value}: {d}");
        }
    }

    // ── triangle strips and fans ────────────────────────────────────────────

    /// A positions-only, non-indexed primitive in one `mode`.
    fn mode_glb(mode: u64, positions: &[[f32; 3]]) -> Vec<u8> {
        let bin: Vec<u8> = positions.iter().flat_map(|p| f32s(p)).collect();
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "mode": mode}]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": positions.len(), "type": "VEC3"}
            ],
            "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": bin.len()}],
            "buffers": [{"byteLength": bin.len()}]
        });
        pack(doc, Some(&bin))
    }

    /// Every facet normal of a mesh, from its winding.
    fn facets(mesh: &Mesh) -> Vec<[f32; 3]> {
        mesh.to_soup().iter().map(Mesh::facet_normal).collect()
    }

    #[test]
    fn expands_a_triangle_strip_keeping_every_face_wound_the_same_way() {
        // A quad as a strip: triangle 1 must swap its first two vertices, or it
        // faces the opposite way from triangle 0 and renders as a hole.
        let quad = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]];
        let mesh = read(&mode_glb(MODE_TRIANGLE_STRIP, &quad)).unwrap();
        assert_eq!(mesh.indices, vec![0, 1, 2, 2, 1, 3]);
        assert_eq!(facets(&mesh), vec![[0.0, 0.0, 1.0]; 2], "alternating faces flipped");
    }

    #[test]
    fn expands_a_triangle_fan_around_its_first_vertex() {
        let fan = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = read(&mode_glb(MODE_TRIANGLE_FAN, &fan)).unwrap();
        assert_eq!(mesh.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(facets(&mesh), vec![[0.0, 0.0, 1.0]; 2]);
    }

    #[test]
    fn a_strip_and_a_fan_of_five_vertices_are_three_triangles_each() {
        let five = [
            [0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0],
        ];
        for mode in [MODE_TRIANGLE_STRIP, MODE_TRIANGLE_FAN] {
            assert_eq!(read(&mode_glb(mode, &five)).unwrap().triangle_count(), 3, "mode {mode}");
        }
    }

    #[test]
    fn a_strip_primitive_beside_a_list_primitive_keeps_both() {
        // The half-missing model the module header exists to prevent: this
        // returned Ok with only the list primitive in it.
        let (mut doc, mut bin) = triangle_doc();
        let strip_start = bin.len();
        bin.extend_from_slice(&f32s(&[2.0, 0.0, 0.0, 3.0, 0.0, 0.0, 2.0, 1.0, 0.0, 3.0, 1.0, 0.0]));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({
            "bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC3"
        }));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 0, "byteOffset": strip_start, "byteLength": 48
        }));
        doc["buffers"][0]["byteLength"] = Value::from(bin.len());
        doc["meshes"][0]["primitives"].as_array_mut().unwrap().push(serde_json::json!({
            "attributes": {"POSITION": 2}, "mode": MODE_TRIANGLE_STRIP
        }));
        let mesh = read(&pack(doc, Some(&bin))).unwrap();
        assert_eq!(mesh.triangle_count(), 3, "the strip must survive the merge");
        assert_eq!(mesh.indices, vec![0, 1, 2, 3, 4, 5, 5, 4, 6]);
    }

    /// 2026-10-12 review, finding 9: a strip's expansion was never charged
    /// and the index ceiling was checked per primitive, so primitives whose
    /// indices are a view-less sparse accessor (4M zeros from a few bytes)
    /// each expanded to 12M indices: twelve of them (3.2 KB) reached 518 MB
    /// before anything refused. The ceiling now counts across primitives,
    /// and the refusal comes at the primitive that crosses it.
    #[test]
    fn strip_expansion_is_charged_and_the_index_ceiling_counts_every_primitive() {
        let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        bin.extend_from_slice(&u32s(&[0, 2]));
        let strip = serde_json::json!({"attributes": {"POSITION": 0}, "indices": 1, "mode": MODE_TRIANGLE_STRIP});
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": vec![strip; 12]}],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"componentType": 5125, "count": 4_000_000, "type": "SCALAR",
                 "sparse": {"count": 1, "indices": {"bufferView": 1, "componentType": 5125}, "values": {"bufferView": 2}}}
            ],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 4},
                {"buffer": 0, "byteOffset": 40, "byteLength": 4}
            ],
            "buffers": [{"byteLength": bin.len()}]
        });
        let d = unwrap_unsupported(read_scene(&pack(doc, Some(&bin)), &NoSidecars).map(|_| ()).unwrap_err());
        assert!(d.contains("primitive 1"), "refused at the primitive that crosses the ceiling, not after all twelve: {d}");
    }

    #[test]
    fn rejects_a_strip_or_fan_too_short_to_be_a_triangle() {
        let two = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]];
        for mode in [MODE_TRIANGLE_STRIP, MODE_TRIANGLE_FAN] {
            let d = unwrap_malformed(read(&mode_glb(mode, &two)).unwrap_err());
            assert!(d.contains("at least 3"), "mode {mode}: {d}");
        }
    }

    /// The flat read still has no points or lines: each mode's primitive,
    /// drawing two of the triangle's vertices, is read and left out.
    #[test]
    fn still_drops_points_and_lines() {
        let (mut doc, bin) = triangle_doc();
        let prim = doc["meshes"][0]["primitives"][0].clone();
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 1, "componentType": 5125, "count": 2, "type": "SCALAR"}));
        for mode in [0, 1, 2, 3] {
            let mut other = prim.clone();
            other["mode"] = Value::from(mode);
            other["indices"] = Value::from(2);
            doc["meshes"][0]["primitives"] = Value::Array(vec![other, prim.clone()]);
            let mesh = read(&pack(doc.clone(), Some(&bin))).unwrap();
            assert_eq!(mesh.triangle_count(), 1, "mode {mode} is not a triangle");
        }
    }

    // ── fuzz-ish guard ──────────────────────────────────────────────────────

    #[test]
    fn never_panics_on_truncations_of_a_valid_file() {
        let glb = triangle_glb();
        for cut in 0..glb.len() {
            let _ = read(&glb[..cut]);
        }
    }

    /// 2026-10-12 review, finding 7: the `data:` test sliced the URI's first
    /// five bytes, which panics when byte 5 falls inside a character — and on
    /// the poll path a panic discards every file the vendor returned. A
    /// non-ASCII file name is an ordinary file name (glTF 2.0 §3.3 lets URIs
    /// carry percent-encoded or IRI text).
    #[test]
    fn a_uri_with_a_multibyte_character_near_its_start_is_a_file_name_not_a_panic() {
        let (mut doc, bin) = triangle_doc();
        doc["buffers"][0]["uri"] = Value::from("モデル.bin");
        let d = unwrap_unsupported(read(&pack(doc.clone(), None)).unwrap_err());
        assert!(d.contains("モデル.bin"), "{d}");
        let files = vec![PackageFile { path: "モデル.bin".into(), bytes: bin }];
        let r = read_scene(&pack(doc, None), &package::MemorySidecars { files: &files }).unwrap();
        assert_eq!(r.scene.triangle_count(), 1);

        let pkg = write_gltf_scene(&super::super::fixtures::textured(), false).unwrap();
        let mut json: Value = serde_json::from_slice(&pkg.files[0].bytes).unwrap();
        let mut files: Vec<PackageFile> = pkg.files[1..].to_vec();
        for (i, image) in json["images"].as_array_mut().unwrap().iter_mut().enumerate() {
            let old = image["uri"].as_str().unwrap().to_string();
            let new = format!("Textür{i}.png");
            files.iter_mut().find(|f| f.path == old).unwrap().path = new.clone();
            image["uri"] = Value::from(new);
        }
        let gltf = serde_json::to_vec(&json).unwrap();
        let with = read_gltf_scene(&gltf, &package::MemorySidecars { files: &files }).unwrap();
        assert!(!with.scene.images.is_empty() && with.warnings.is_empty(), "{:?}", with.warnings);
        let buffers_only: Vec<PackageFile> = files.iter().filter(|f| !f.path.starts_with("Textür")).cloned().collect();
        let without = read_gltf_scene(&gltf, &package::MemorySidecars { files: &buffers_only }).unwrap();
        assert!(without.scene.images.is_empty(), "a texture not supplied is dropped");
        assert!(without.warnings.iter().any(|w| w.contains("Textür")), "{:?}", without.warnings);
    }

    /// 2026-10-12 review, finding 29: material and light extensions the
    /// scene does not model were dropped without a word, even when the file
    /// listed them as required — so glass turned opaque and lights vanished,
    /// and nothing in the conversion's report said so.
    ///
    /// Since mesh-gaps item 4 the layer extensions (transmission among them)
    /// are carried, so material variants stand for the material extensions
    /// still dropped. Since item 5, `KHR_lights_punctual` is carried too.
    #[test]
    fn extensions_the_scene_does_not_carry_are_named_when_read() {
        let (mut doc, bin) = triangle_doc();
        doc["materials"] = serde_json::json!([{ "extensions": { "KHR_materials_transmission": { "transmissionFactor": 1.0 } } }]);
        doc["meshes"][0]["primitives"][0]["material"] = Value::from(0);
        doc["extensionsUsed"] =
            serde_json::json!(["KHR_materials_variants", "KHR_lights_punctual", "KHR_materials_transmission", "KHR_texture_transform"]);
        doc["extensionsRequired"] = serde_json::json!(["KHR_materials_variants"]);
        let r = read_scene(&pack(doc, Some(&bin)), &NoSidecars).unwrap();
        assert!(r.warnings.iter().any(|w| w.contains("KHR_materials_variants")), "KHR_materials_variants: {:?}", r.warnings);
        for ext in ["KHR_lights_punctual", "KHR_materials_transmission", "KHR_texture_transform"] {
            assert!(!r.warnings.iter().any(|w| w.contains(ext)), "{ext} is carried, so not lost: {:?}", r.warnings);
        }
        assert_eq!(r.scene.materials[0].transmission, Some(Transmission { factor: 1.0, texture: None }));
    }

    /// glTF core `cameras` were dropped without a word, against the module's
    /// never-silent rule (mesh-gaps item 5, Task 1). Since Task 3 carries
    /// them, the camera the reader still drops is one that breaks a bound,
    /// here a perspective `znear` of 0: it is named in exactly one warning,
    /// whole, the file still reads, and its node stays, placing nothing.
    #[test]
    fn glb_names_cameras_it_drops() {
        let (mut doc, bin) = triangle_doc();
        doc["cameras"] = serde_json::json!([{ "name": "shot", "type": "perspective", "perspective": { "yfov": 0.8, "znear": 0 } }]);
        doc["nodes"][0]["camera"] = Value::from(0);
        let r = read_scene(&pack(doc, Some(&bin)), &NoSidecars).expect("a camera never refuses the file");
        assert_eq!(r.warnings, [r#"camera "shot" is dropped: its znear 0 is not positive"#]);
        assert!(r.scene.cameras.is_empty(), "{:?}", r.scene.cameras);
        assert_eq!((r.scene.nodes.len(), r.scene.nodes[0].camera, r.scene.triangle_count()), (1, None, 1), "its node stays, placing nothing");
    }

    // ── lights and cameras ──────────────────────────────────────────────────

    // The JSON below is hand-written from KHR_lights_punctual's schema
    // (`light.schema.json`, `light.spot.schema.json`,
    // `node.KHR_lights_punctual.schema.json`) and glTF 2.0's camera schemas
    // (`camera.schema.json`, `camera.perspective.schema.json`,
    // `camera.orthographic.schema.json`):
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_lights_punctual/schema
    // https://github.com/KhronosGroup/glTF/tree/main/specification/2.0/schema

    /// GLB and glTF, each written both ways (`self_contained`).
    const GLTF_WRITE_MODES: [(MeshFormat, bool); 4] =
        [(MeshFormat::Glb, true), (MeshFormat::Glb, false), (MeshFormat::Gltf, true), (MeshFormat::Gltf, false)];

    /// The triangle (node 0) with `nodes` beside it at the root, and `lights`
    /// (`KHR_lights_punctual`) and `cameras` in the document.
    fn lit_triangle(lights: Vec<Value>, cameras: Vec<Value>, nodes: Vec<Value>) -> Vec<u8> {
        let (mut doc, bin) = triangle_doc();
        doc["extensions"] = serde_json::json!({ "KHR_lights_punctual": { "lights": lights } });
        doc["extensionsUsed"] = serde_json::json!(["KHR_lights_punctual"]);
        doc["cameras"] = Value::Array(cameras);
        doc["scenes"][0]["nodes"] = (0..=nodes.len()).collect();
        doc["nodes"].as_array_mut().unwrap().extend(nodes);
        pack(doc, Some(&bin))
    }

    /// `fixtures::staged()` through GLB and glTF in both write modes: its
    /// lights and cameras are written as hand-written here from the schemas
    /// above, each node placing its own; every one reads back equal, field
    /// for field; and at rest and at every sample time each stands, faces
    /// and projects as the fixture's does, within 1e-5 ([`lightcam`]).
    /// Nothing is reported lost, and the read warns of nothing.
    #[test]
    fn every_light_and_camera_round_trips() {
        let scene = super::super::fixtures::staged();
        let lights = serde_json::json!([
            { "name": "sun", "type": "directional", "color": [1.0, 0.875, 0.75], "intensity": 3.0 },
            { "name": "bulb", "type": "point", "color": [1.0, 0.75, 0.5], "intensity": 20.0, "range": 10.0 },
            {
                "name": "spot", "type": "spot", "color": [0.5, 0.75, 1.0], "intensity": 40.0,
                "spot": { "innerConeAngle": 0.2f32, "outerConeAngle": 0.6f32 }
            }
        ]);
        let cameras = serde_json::json!([
            { "name": "persp_camera", "type": "perspective", "perspective": { "yfov": 0.8f32, "aspectRatio": 1.5, "znear": 0.125, "zfar": 64.0 } },
            { "name": "ortho_camera", "type": "orthographic", "orthographic": { "xmag": 3.0, "ymag": 2.0, "znear": 0.0625, "zfar": 20.0 } }
        ]);
        let placed = [
            ("persp_camera_node", "camera", serde_json::json!(0)),
            ("ortho_camera_node", "camera", serde_json::json!(1)),
            ("sun_node", "extensions", serde_json::json!({ "KHR_lights_punctual": { "light": 0 } })),
            ("bulb_node", "extensions", serde_json::json!({ "KHR_lights_punctual": { "light": 1 } })),
            ("spot_node", "extensions", serde_json::json!({ "KHR_lights_punctual": { "light": 2 } })),
        ];
        for (fmt, self_contained) in GLTF_WRITE_MODES {
            let ctx = format!("{fmt}{}", if self_contained { "" } else { " (package)" });
            let (doc, losses, back) = layer_round_trip(&scene, fmt, self_contained);
            assert_eq!(doc["extensions"]["KHR_lights_punctual"]["lights"], lights, "{ctx}: the lights");
            assert_eq!(doc["cameras"], cameras, "{ctx}: the cameras");
            for (name, key, want) in &placed {
                let node = doc["nodes"].as_array().and_then(|ns| ns.iter().find(|n| n["name"] == *name));
                assert_eq!(node.map(|n| &n[key]), Some(want), "{ctx}: {name}'s {key}");
            }
            assert!(losses.is_empty(), "{ctx}: {losses:?}");
            assert!(back.warnings.is_empty(), "{ctx}: {:?}", back.warnings);
            assert_eq!(back.scene.lights, scene.lights, "{ctx}: every light, field for field");
            assert_eq!(back.scene.cameras, scene.cameras, "{ctx}: every camera, field for field");
            lightcam::assert_same_lights_and_cameras(&scene, &back.scene, 1e-5, &ctx);
        }
    }

    /// A reader without KHR_lights_punctual draws the model by its own
    /// light, which is the extension's intent: a scene with a light lists it
    /// in `extensionsUsed` and never in `extensionsRequired`, even when the
    /// source required it, and a scene without one lists it nowhere and
    /// writes no lights.
    #[test]
    fn lights_are_never_required() {
        fn names<'a>(doc: &'a Value, key: &str) -> Vec<&'a str> {
            doc[key].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
        }
        let staged = super::super::fixtures::staged();
        let source = patched(&staged, |doc| doc["extensionsRequired"] = serde_json::json!(["KHR_lights_punctual"]));
        let read = read_scene(&source, &NoSidecars).unwrap();
        assert_eq!(read.scene.lights, staged.lights, "the source's lights");
        let mut dark = staged.clone();
        dark.lights.clear();
        for n in dark.nodes.iter_mut() {
            n.light = None;
        }
        for (fmt, self_contained) in GLTF_WRITE_MODES {
            let (doc, _, _) = layer_round_trip(&read.scene, fmt, self_contained);
            let (used, required) = (names(&doc, "extensionsUsed"), names(&doc, "extensionsRequired"));
            assert!(used.contains(&"KHR_lights_punctual"), "{fmt}: not in extensionsUsed {used:?}");
            assert!(!required.contains(&"KHR_lights_punctual"), "{fmt}: in extensionsRequired {required:?}");
            let (doc, _, back) = layer_round_trip(&dark, fmt, self_contained);
            assert!(!names(&doc, "extensionsUsed").contains(&"KHR_lights_punctual"), "{fmt}: no light, yet {:?}", doc["extensionsUsed"]);
            assert_eq!(doc.get("extensions"), None, "{fmt}: no light, yet a lights array");
            assert_eq!(back.scene.cameras, staged.cameras, "{fmt}: the cameras stay");
        }
    }

    /// Review Focus 1: glTF ignores a skinned mesh node's transform, so the
    /// writer moves every skinned mesh to a root of its own
    /// ([`hoist_skinned_meshes`]). A light and a camera on the node that held
    /// the mesh, and those on the joints the clip bends, keep their nodes,
    /// and with them their world frames, at rest and at every sample time.
    #[test]
    fn a_light_on_a_skinned_node_survives_hoisting() {
        let mut s = super::super::fixtures::skinned();
        let at = |s: &Scene, name: &str| s.nodes.iter().position(|n| n.name.as_deref() == Some(name)).unwrap();
        let (tube, knee, ankle) = (at(&s, "tube_node"), at(&s, "knee"), at(&s, "ankle"));
        // Off the identity root, so that the mesh is hoisted.
        s.nodes[tube].transform =
            Transform::Trs { translation: [1.5, 0.25, -0.5], rotation: math::quat_from_axis_angle([0.0, 1.0, 0.0], 0.5), scale: [1.0; 3] };
        s.lights = vec![
            Light { name: Some("tube_lamp".into()), kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, color: [1.0; 3], intensity: 40.0, range: None },
            Light { name: Some("knee_lamp".into()), kind: LightKind::Point, color: [1.0, 0.5, 0.25], intensity: 20.0, range: Some(5.0) },
        ];
        s.cameras = vec![
            Camera { name: Some("tube_eye".into()), projection: Projection::Perspective { yfov: 0.8, aspect: Some(1.5), znear: 0.125, zfar: None } },
            Camera { name: Some("ankle_eye".into()), projection: Projection::Orthographic { xmag: 1.0, ymag: 1.0, znear: 0.0, zfar: 10.0 } },
        ];
        (s.nodes[tube].light, s.nodes[tube].camera) = (Some(0), Some(0));
        s.nodes[knee].light = Some(1);
        s.nodes[ankle].camera = Some(1);
        s.validate().unwrap();
        for (fmt, self_contained) in GLTF_FORMS {
            let ctx = format!("{fmt}{}", if self_contained { "" } else { " (package)" });
            let (_, losses, back) = layer_round_trip(&s, fmt, self_contained);
            let hoisted = back.scene.nodes.iter().find(|n| n.name.as_deref() == Some("tube_node_skinned"));
            assert!(hoisted.is_some_and(|n| n.mesh.is_some() && n.skin.is_some()), "{ctx}: the skinned mesh is hoisted to a root of its own");
            assert_eq!(back.scene.nodes[at(&back.scene, "tube_node")].mesh, None, "{ctx}: and leaves its node");
            lightcam::assert_same_lights_and_cameras(&s, &back.scene, 1e-5, &ctx);
            assert!(losses.is_empty() && back.warnings.is_empty(), "{ctx}: {losses:?} {:?}", back.warnings);
        }
    }

    /// Review Focus 4 (the plan's Ruling R5): a spot whose inner cone equals
    /// its outer one has a hard edge, which glTF forbids and FBX, Collada and
    /// USD files carry routinely. It is softened, not dropped: its inner cone
    /// moves inside the outer one by 1e-4 of it, with one warning naming the
    /// light and saying its hard edge was softened, though two nodes place
    /// it: a light is read once, however many nodes place it. The softened
    /// spot holds the bounds, comes back from a glTF round trip unchanged and
    /// unwarned, and stays `inner < outer` through the rounding that FBX's
    /// and USD's degrees add.
    #[test]
    fn a_hard_edged_spot_is_nudged_and_warned() {
        let hard = serde_json::json!({ "name": "stage_spot", "type": "spot", "intensity": 40, "spot": { "innerConeAngle": 0.5, "outerConeAngle": 0.5 } });
        let node = |name: &str| serde_json::json!({ "name": name, "extensions": { "KHR_lights_punctual": { "light": 0 } } });
        let r = read_scene(&lit_triangle(vec![hard], vec![], vec![node("spot_node"), node("spot_node_2")]), &NoSidecars)
            .expect("a light never refuses the file");
        assert_eq!(r.scene.lights.len(), 1, "softened, not dropped: {:?}", r.warnings);
        let LightKind::Spot { inner_cone, outer_cone } = r.scene.lights[0].kind else { panic!("{:?}", r.scene.lights[0]) };
        assert_eq!(outer_cone, 0.5);
        let gap = (outer_cone - inner_cone) / outer_cone;
        assert!((0.99e-4..=1.01e-4).contains(&gap), "inner cone {inner_cone}: softened by {gap} of the outer cone");
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        let w = &r.warnings[0];
        assert!(w.starts_with(r#"light "stage_spot" "#) && w.contains("hard edge") && w.contains("softened"), "{w}");
        assert_eq!(r.scene.lights[0].check(), Ok(()));
        assert_eq!((r.scene.nodes[1].light, r.scene.nodes[2].light), (Some(0), Some(0)), "both nodes place the one light");
        for (fmt, self_contained) in GLTF_FORMS {
            let (_, _, back) = layer_round_trip(&r.scene, fmt, self_contained);
            assert_eq!(back.scene.lights, r.scene.lights, "{fmt}: the softened spot comes back as it is");
            assert!(back.warnings.is_empty(), "{fmt}: softened once only: {:?}", back.warnings);
        }
        // FBX writes full cones in degrees (f64) and USD half-angles in
        // degrees (f32): each rounds an angle on the way out and back.
        let fbx = |a: f32| {
            let full = 2.0 * f64::from(a).to_degrees();
            (full / 2.0).to_radians() as f32
        };
        let usd = |a: f32| f64::from(f64::from(a).to_degrees() as f32).to_radians() as f32;
        for outer in [1e-3f32, 0.2, 0.5, 1.0, std::f32::consts::FRAC_PI_2] {
            let mut l = Light { name: None, kind: LightKind::Spot { inner_cone: outer, outer_cone: outer }, color: [1.0; 3], intensity: 1.0, range: None };
            assert!(l.soften_hard_edge().is_some(), "{outer}");
            let LightKind::Spot { inner_cone, outer_cone } = l.kind else { unreachable!() };
            assert!(fbx(inner_cone) < fbx(outer_cone) && usd(inner_cone) < usd(outer_cone), "{outer}: {inner_cone} through the degrees");
            assert_eq!(l.check(), Ok(()), "{outer}");
        }
    }

    /// The spec's "readers never refuse a file over a light or camera"
    /// (Review Focus 6, glTF's half). A light or camera that breaks a bound
    /// (cones out of order, a perspective `znear` of 0, an intensity past
    /// f32, which reads as infinite), that does not read (a type glTF does
    /// not define, a value that is no number, …), that a node places past
    /// the file's array or by no index, or that no node of the scene places,
    /// is dropped with one warning naming it and why, whole. The file still
    /// reads, every node stays, and the good light and camera beside it are
    /// kept, renumbered. A directional light's `range`, which glTF ignores,
    /// is ignored with a warning, and the light kept.
    #[test]
    fn a_bad_light_or_camera_is_dropped_not_refused() {
        use serde_json::json;
        let light = |i: Value| json!({ "name": "bad_node", "extensions": { "KHR_lights_punctual": { "light": i } } });
        let camera = |i: Value| json!({ "name": "bad_node", "camera": i });
        let lone = json!({ "name": "bad_node" });
        // (what, a bad light the file lists first, a bad camera it lists
        // first, node 1 beside the good ones, the one warning)
        let cases: Vec<(&str, Option<Value>, Option<Value>, Value, &str)> = vec![
            (
                "cones out of order",
                Some(json!({ "name": "lamp", "type": "spot", "spot": { "innerConeAngle": 0.7, "outerConeAngle": 0.6 } })),
                None,
                light(json!(0)),
                r#"light "lamp" is dropped: its inner cone 0.7 is not below its outer cone 0.6"#,
            ),
            (
                "a perspective znear of 0",
                None,
                Some(json!({ "name": "shot", "type": "perspective", "perspective": { "yfov": 0.8, "znear": 0 } })),
                camera(json!(0)),
                r#"camera "shot" is dropped: its znear 0 is not positive"#,
            ),
            (
                "an intensity past f32",
                Some(json!({ "name": "flare", "type": "point", "intensity": 1e39 })),
                None,
                light(json!(0)),
                r#"light "flare" is dropped: its intensity inf is not finite"#,
            ),
            ("a light past the lights", None, None, light(json!(4)), "node 1 places light 4, past the file's 1 light(s); the node is kept without it"),
            ("a camera past the cameras", None, None, camera(json!(2)), "node 1 places camera 2, past the file's 1 camera(s); the node is kept without it"),
            ("a light no node places", Some(json!({ "type": "point" })), None, lone.clone(), "light 0 is placed by no node of the scene, so it is dropped"),
            (
                "a camera no node places",
                None,
                Some(json!({ "name": "spare", "type": "orthographic", "orthographic": { "xmag": 1, "ymag": 1, "znear": 0, "zfar": 10 } })),
                lone.clone(),
                r#"camera "spare" is placed by no node of the scene, so it is dropped"#,
            ),
            ("a light that is no object", Some(json!(5)), None, light(json!(0)), "light 0 is dropped: it is 5, which is not an object"),
            ("a light with no type", Some(json!({ "name": "lamp" })), None, light(json!(0)), r#"light "lamp" is dropped: it has no type"#),
            (
                "a light type glTF does not define",
                Some(json!({ "name": "panel", "type": "area" })),
                None,
                light(json!(0)),
                r#"light "panel" is dropped: its type "area" is not directional, point or spot"#,
            ),
            (
                "an intensity that is no number",
                Some(json!({ "name": "lamp", "type": "point", "intensity": "bright" })),
                None,
                light(json!(0)),
                r#"light "lamp" is dropped: its intensity is "bright", which is not a number"#,
            ),
            (
                "a colour that is not 3 numbers",
                Some(json!({ "name": "lamp", "type": "point", "color": [1, 1] })),
                None,
                light(json!(0)),
                r#"light "lamp" is dropped: its color is [1,1], which is not 3 numbers"#,
            ),
            (
                "a spot that is no object",
                Some(json!({ "name": "lamp", "type": "spot", "spot": 0.5 })),
                None,
                light(json!(0)),
                r#"light "lamp" is dropped: its spot is 0.5, which is not an object"#,
            ),
            (
                "a camera type glTF does not define",
                None,
                Some(json!({ "name": "fish", "type": "fisheye" })),
                camera(json!(0)),
                r#"camera "fish" is dropped: its type "fisheye" is not perspective or orthographic"#,
            ),
            (
                "a perspective with no yfov",
                None,
                Some(json!({ "name": "shot", "type": "perspective", "perspective": { "znear": 0.1 } })),
                camera(json!(0)),
                r#"camera "shot" is dropped: its perspective has no yfov"#,
            ),
            (
                "an orthographic that is no object",
                None,
                Some(json!({ "name": "plan", "type": "orthographic", "orthographic": [] })),
                camera(json!(0)),
                r#"camera "plan" is dropped: its orthographic is [], which is not an object"#,
            ),
            ("a camera by no index", None, None, camera(json!("x")), r#"node 1 camera is "x", which is not an index; the node is kept without it"#),
            ("a light by no index", None, None, light(json!(-1)), "node 1 KHR_lights_punctual light is -1, which is not an index; the node is kept without it"),
            (
                "a node extension that names no light",
                None,
                None,
                json!({ "name": "bad_node", "extensions": { "KHR_lights_punctual": 5 } }),
                "node 1 KHR_lights_punctual is 5, which names no light; the node is kept without it",
            ),
        ];
        let good_light = json!({ "name": "good", "type": "spot", "intensity": 40, "spot": { "innerConeAngle": 0.2, "outerConeAngle": 0.6 } });
        let good_camera = json!({ "name": "eye", "type": "perspective", "perspective": { "yfov": 0.8, "znear": 0.125 } });
        for (what, bad_light, bad_camera, bad_node, warning) in cases {
            let lights: Vec<Value> = bad_light.into_iter().chain([good_light.clone()]).collect();
            let cameras: Vec<Value> = bad_camera.into_iter().chain([good_camera.clone()]).collect();
            let good_nodes = [
                json!({ "name": "lit_node", "extensions": { "KHR_lights_punctual": { "light": lights.len() - 1 } } }),
                json!({ "name": "eye_node", "camera": cameras.len() - 1 }),
            ];
            let file = lit_triangle(lights, cameras, std::iter::once(bad_node).chain(good_nodes).collect());
            let r = read_scene(&file, &NoSidecars).unwrap_or_else(|e| panic!("{what}: refused: {e}"));
            assert_eq!(r.warnings, [warning], "{what}");
            let names: Vec<Option<&str>> = r.scene.nodes.iter().map(|n| n.name.as_deref()).collect();
            assert_eq!(names, [None, Some("bad_node"), Some("lit_node"), Some("eye_node")], "{what}: every node stays");
            assert_eq!((r.scene.nodes[1].light, r.scene.nodes[1].camera), (None, None), "{what}: the bad node places nothing");
            assert_eq!((r.scene.nodes[2].light, r.scene.nodes[3].camera), (Some(0), Some(0)), "{what}: the good ones, renumbered");
            let lights: Vec<Option<&str>> = r.scene.lights.iter().map(|l| l.name.as_deref()).collect();
            let cameras: Vec<Option<&str>> = r.scene.cameras.iter().map(|c| c.name.as_deref()).collect();
            assert_eq!((lights, cameras), (vec![Some("good")], vec![Some("eye")]), "{what}: the good ones are kept");
        }
        let sun = json!({ "name": "sun", "type": "directional", "intensity": 3, "range": 10 });
        let node = json!({ "name": "sun_node", "extensions": { "KHR_lights_punctual": { "light": 0 } } });
        let r = read_scene(&lit_triangle(vec![sun], vec![], vec![node]), &NoSidecars).expect("a light never refuses the file");
        assert_eq!(r.warnings, [r#"light "sun" is directional, so its range 10 is ignored"#]);
        let kept = Light { name: Some("sun".into()), kind: LightKind::Directional, color: [1.0; 3], intensity: 3.0, range: None };
        assert_eq!((r.scene.lights, r.scene.nodes[1].light), (vec![kept], Some(0)));
        // A name past 60 characters is quoted cut, never whole (Ruling A45).
        let long = json!({ "name": "x".repeat(100_000), "type": "area" });
        let r = read_scene(&lit_triangle(vec![long], vec![], vec![light(json!(0))]), &NoSidecars).expect("a light never refuses the file");
        assert_eq!(r.warnings, [format!(r#"light "{}… is dropped: its type "area" is not directional, point or spot"#, "x".repeat(59))]);
    }

    /// What bounds the warnings for lights no node places: the node budget,
    /// charged before each warning is made. A file of more lights than a
    /// scene can place (`MAX_NODES`) has that many named, one warning each,
    /// and the rest counted in one more, never a warning per light.
    #[test]
    fn unplaced_lights_are_named_as_far_as_the_node_budget() {
        let n = super::super::scene::MAX_NODES;
        let lights = vec![serde_json::json!({ "type": "point" }); n + 2];
        let r = read_scene(&lit_triangle(lights, vec![], vec![]), &NoSidecars).expect("lights never refuse the file");
        assert_eq!(r.warnings.len(), n + 1);
        assert_eq!(r.warnings[0], "light 0 is placed by no node of the scene, so it is dropped");
        assert_eq!(r.warnings[n - 1], format!("light {} is placed by no node of the scene, so it is dropped", n - 1));
        assert_eq!(r.warnings[n], "2 more light(s) are placed by no node of the scene, so they are dropped");
        assert!(r.scene.lights.is_empty());
    }

    /// Task 2's carried note: an outer cone of f32 π/2 holds the bounds
    /// (`Light::check`), yet written as that f32 it is past
    /// KHR_lights_punctual's maximum, the f64 π/2 (`light.spot.schema.json`),
    /// so the writer clamps it there. It reads back as the same f32.
    #[test]
    fn a_right_angled_spot_is_written_within_the_schema() {
        let mut s = super::super::fixtures::staged();
        s.lights[2].kind = LightKind::Spot { inner_cone: 0.2, outer_cone: std::f32::consts::FRAC_PI_2 };
        for (fmt, self_contained) in GLTF_FORMS {
            let (doc, _, back) = layer_round_trip(&s, fmt, self_contained);
            let outer = doc["extensions"]["KHR_lights_punctual"]["lights"][2]["spot"]["outerConeAngle"].as_f64();
            assert_eq!(outer, Some(std::f64::consts::FRAC_PI_2), "{fmt}: the schema's maximum");
            assert_eq!(back.scene.lights[2].kind, s.lights[2].kind, "{fmt}: read back as f32 π/2");
        }
    }

    /// Ruling R32 in glTF: a spot whose outer cone is past π/2 is wider than
    /// a hemisphere (and past the schema's maximum). It lights nearly every
    /// way, so it is read as a point light, its range kept and its cones
    /// gone, with one warning naming it, not dropped. A spot of π/2 itself,
    /// a hemisphere, stays a spot.
    #[test]
    fn a_spot_wider_than_a_hemisphere_reads_as_a_point_light() {
        use serde_json::json;
        let wide = json!({ "name": "flood", "type": "spot", "intensity": 40, "range": 5, "spot": { "innerConeAngle": 0.5, "outerConeAngle": 2.0 } });
        let edge = json!({ "name": "edge", "type": "spot", "intensity": 40, "spot": { "innerConeAngle": 0.5, "outerConeAngle": std::f64::consts::FRAC_PI_2 } });
        let node = |name: &str, i: usize| json!({ "name": name, "extensions": { "KHR_lights_punctual": { "light": i } } });
        let file = lit_triangle(vec![wide, edge], vec![], vec![node("flood_node", 0), node("edge_node", 1)]);
        let r = read_scene(&file, &NoSidecars).expect("a light never refuses the file");
        let flood = Light { name: Some("flood".into()), kind: LightKind::Point, color: [1.0; 3], intensity: 40.0, range: Some(5.0) };
        assert_eq!(r.scene.lights.first(), Some(&flood), "{:?}", r.warnings);
        assert_eq!(r.scene.nodes[1].light, Some(0), "its node places it");
        let edge = r.scene.lights.get(1).map(|l| l.kind);
        assert!(matches!(edge, Some(LightKind::Spot { outer_cone, .. }) if outer_cone == std::f32::consts::FRAC_PI_2), "π/2 stays a spot: {edge:?}");
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with(r#"light "flood" has an outer cone of 2 rad"#) && r.warnings[0].contains("point light"), "{:?}", r.warnings);
    }

    /// Ruling R8 in glTF: a colour past 1 breaks KHR_lights_punctual's
    /// maximum, and its peak holds part of the brightness, as FBX, Collada
    /// and USD colours do. So the reader moves the peak into the intensity
    /// and keeps the light, with one warning naming it, since the file's
    /// own values were not glTF's (the others' formats hold colour ×
    /// intensity legally, so they say nothing).
    #[test]
    fn an_over_range_colour_moves_into_the_intensity() {
        let hot = serde_json::json!({ "name": "hot", "type": "point", "color": [2.0, 1.0, 0.5], "intensity": 3.0 });
        let node = serde_json::json!({ "name": "hot_node", "extensions": { "KHR_lights_punctual": { "light": 0 } } });
        let r = read_scene(&lit_triangle(vec![hot], vec![], vec![node]), &NoSidecars).expect("a light never refuses the file");
        let want = Light { name: Some("hot".into()), kind: LightKind::Point, color: [1.0, 0.5, 0.25], intensity: 6.0, range: None };
        assert_eq!((r.scene.lights.clone(), r.scene.nodes[1].light), (vec![want], Some(0)), "{:?}", r.warnings);
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].starts_with(r#"light "hot" has a colour"#) && r.warnings[0].contains("intensity"), "{:?}", r.warnings);
    }

    /// The scene keeps the file's camera order, whatever order its nodes
    /// place them in: GLTFLoader lists `gltf.cameras` in array order, and
    /// three.js apps and the harness take the first as the file's camera,
    /// so a conversion must not change which one that is.
    #[test]
    fn cameras_keep_the_files_order() {
        let cam = |name: &str| serde_json::json!({ "name": name, "type": "perspective", "perspective": { "yfov": 0.8, "znear": 0.125 } });
        let node = |name: &str, c: usize| serde_json::json!({ "name": name, "camera": c });
        let file = lit_triangle(vec![], vec![cam("first"), cam("second")], vec![node("b_node", 1), node("a_node", 0)]);
        let r = read_scene(&file, &NoSidecars).unwrap();
        let names = |s: &Scene| s.cameras.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&r.scene), [Some("first".to_string()), Some("second".to_string())], "{:?}", r.warnings);
        assert_eq!((r.scene.nodes[1].camera, r.scene.nodes[2].camera), (Some(1), Some(0)), "each node still places its own");
        for (fmt, self_contained) in GLTF_FORMS {
            let (doc, _, back) = layer_round_trip(&r.scene, fmt, self_contained);
            assert_eq!(doc["cameras"][0]["name"], "first", "{fmt}");
            assert_eq!(names(&back.scene), names(&r.scene), "{fmt}");
        }
    }

    /// Ruling R13 in glTF's `prepare`: a directional light never carries a
    /// range. glTF applies one to point and spot lights only, and the
    /// writer leaves a directional one out, so `prepare` clears it, as
    /// FBX's, Collada's and USD's do: the expected scene is the one that
    /// reads back. It goes unreported, as no reader applies it.
    #[test]
    fn prepare_drops_a_directional_range_as_the_writer_does() {
        let mut s = super::super::fixtures::staged();
        s.lights[0].range = Some(5.0);
        for (fmt, self_contained) in GLTF_FORMS {
            let prepared = fmt.prepare(&s, super::super::WriteOptions { self_contained, ..Default::default() }).unwrap();
            assert_eq!((prepared.lights[0].range, prepared.lights[1].range), (None, Some(10.0)), "{fmt}: the sun's goes, the bulb's stays");
            let (_, losses, back) = layer_round_trip(&s, fmt, self_contained);
            assert!(losses.is_empty(), "{fmt}: {losses:?}");
            assert_eq!(back.scene.lights, prepared.lights, "{fmt}: prepare models what reads back");
        }
    }

    /// The light and camera reader through the overview's hostile-input bar
    /// ([`layer_sweep`]): `fixtures::staged()` as a GLB, read whole, where
    /// it must carry its three lights and two cameras, then swept with every
    /// bit from its `cameras` array to its `nodes`, which place them (its
    /// `KHR_lights_punctual` lights between), among the single flips. Every
    /// reading is `Ok` or `Err`, never a panic.
    #[test]
    fn glb_light_and_camera_inputs_survive_truncation_and_bit_flips() {
        let name = "staged as GLB";
        let file = one_file(&super::super::fixtures::staged(), MeshFormat::Glb);
        let whole = super::super::read_scene(&file, MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((whole.scene.lights.len(), whole.scene.cameras.len()), (3, 2), "{name} itself carries every light and camera");
        layer_sweep(name, &file, span(name, &file, r#""cameras":"#, r#""scene":"#), |bytes| {
            let _ = super::super::read_scene(bytes, MeshFormat::Glb, &NoSidecars);
        });
    }

    // ── material layers ─────────────────────────────────────────────────────

    // glTF's PBR layer extensions. Each one's JSON below (its property names
    // and defaults) is hand-written from its schema, fetched from
    // KhronosGroup/glTF at commit 9d23a5a5 on 2026-11-04:
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_clearcoat/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_transmission/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_volume/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_ior/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_specular/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_sheen/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_iridescence/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_anisotropy/schema
    // https://github.com/KhronosGroup/glTF/tree/main/extensions/2.0/Khronos/KHR_materials_dispersion/schema
    const LAYER_EXTENSIONS: [&str; 9] = [
        "KHR_materials_clearcoat",
        "KHR_materials_transmission",
        "KHR_materials_volume",
        "KHR_materials_ior",
        "KHR_materials_specular",
        "KHR_materials_sheen",
        "KHR_materials_iridescence",
        "KHR_materials_anisotropy",
        "KHR_materials_dispersion",
    ];

    /// GLB, and glTF as one file and as a package (`self_contained`).
    const GLTF_FORMS: [(MeshFormat, bool); 3] = [(MeshFormat::Glb, true), (MeshFormat::Gltf, true), (MeshFormat::Gltf, false)];

    /// `scene` written as `fmt` through the conversion API: the glTF document,
    /// the write's losses, and what reading the written files back gives.
    fn layer_round_trip(scene: &Scene, fmt: MeshFormat, self_contained: bool) -> (Value, Vec<super::super::Loss>, super::super::SceneRead) {
        let w = super::super::write_scene(scene, fmt, super::super::WriteOptions { self_contained, ..Default::default() }).unwrap();
        let files = &w.package.files;
        let doc = if fmt == MeshFormat::Glb { json_of(&files[0].bytes) } else { serde_json::from_slice(&files[0].bytes).unwrap() };
        let back = super::super::read_scene(&files[0].bytes, fmt, &package::MemorySidecars { files }).unwrap();
        (doc, w.losses, back)
    }

    /// `fixtures::layered()` through GLB and glTF: every material comes back
    /// equal, field for field, nothing is reported lost, and each material's
    /// `extensions` is exactly the JSON hand-written here from the schemas
    /// above, with every field at its default left out.
    #[test]
    fn every_pbr_extension_round_trips() {
        let scene = super::super::fixtures::layered();
        let expected = [
            (
                "car_paint",
                serde_json::json!({
                    "KHR_materials_clearcoat": {
                        "clearcoatFactor": 1.0,
                        "clearcoatTexture": { "index": 0 },
                        "clearcoatRoughnessFactor": 0.1f32,
                        "clearcoatRoughnessTexture": { "index": 1 },
                        "clearcoatNormalTexture": { "index": 0, "scale": 0.5 }
                    }
                }),
            ),
            (
                "glass",
                serde_json::json!({
                    "KHR_materials_transmission": { "transmissionFactor": 1.0 },
                    "KHR_materials_volume": {
                        "thicknessFactor": 0.4f32,
                        "thicknessTexture": { "index": 1 },
                        "attenuationDistance": 1.5,
                        "attenuationColor": [0.9f32, 0.55f32, 0.2f32]
                    },
                    "KHR_materials_ior": { "ior": 1.52f32 },
                    "KHR_materials_dispersion": { "dispersion": 0.3f32 }
                }),
            ),
            (
                "velvet",
                serde_json::json!({
                    "KHR_materials_sheen": {
                        "sheenColorFactor": [0.9f32, 0.6f32, 0.8f32],
                        "sheenColorTexture": { "index": 0 },
                        "sheenRoughnessFactor": 0.4f32
                    }
                }),
            ),
            (
                "plastic",
                serde_json::json!({
                    "KHR_materials_specular": {
                        "specularFactor": 0.6f32,
                        "specularColorFactor": [1.0, 0.85f32, 0.7f32],
                        "specularColorTexture": { "index": 1 }
                    },
                    "KHR_materials_ior": { "ior": 1.6f32 }
                }),
            ),
            (
                "soap_film",
                serde_json::json!({
                    "KHR_materials_iridescence": {
                        "iridescenceFactor": 1.0,
                        "iridescenceIor": 1.33f32,
                        "iridescenceThicknessMinimum": 200.0,
                        "iridescenceThicknessMaximum": 500.0,
                        "iridescenceThicknessTexture": { "index": 0 }
                    }
                }),
            ),
            (
                "brushed_steel",
                serde_json::json!({
                    "KHR_materials_anisotropy": { "anisotropyStrength": 0.7f32, "anisotropyRotation": 0.6f32 }
                }),
            ),
            (
                "lacquer",
                serde_json::json!({
                    "KHR_materials_clearcoat": { "clearcoatFactor": 1.0, "clearcoatRoughnessFactor": 0.2f32 },
                    "KHR_materials_ior": { "ior": 1.45f32 }
                }),
            ),
        ];
        assert_eq!(scene.materials.len(), expected.len());
        let pixels = |s: &Scene| s.textures.iter().map(|t| s.images[t.image].data.clone()).collect::<Vec<_>>();
        for (fmt, self_contained) in GLTF_FORMS {
            let ctx = format!("{fmt}{}", if self_contained { "" } else { " (package)" });
            let (doc, losses, back) = layer_round_trip(&scene, fmt, self_contained);
            for (mi, (name, want)) in expected.iter().enumerate() {
                assert_eq!(doc["materials"][mi]["name"], *name, "{ctx}: material {mi}");
                assert_eq!(doc["materials"][mi]["extensions"], *want, "{ctx}: {name}'s extensions");
            }
            assert_eq!(back.scene.materials, scene.materials, "{ctx}: every material, field for field");
            assert_eq!(pixels(&back.scene), pixels(&scene), "{ctx}: each texture's image");
            assert!(back.warnings.is_empty(), "{ctx}: {:?}", back.warnings);
            assert!(losses.is_empty(), "{ctx}: {losses:?}");
        }
    }

    /// A reader without a layer extension draws the base layer, which is the
    /// extension's intent: every layer written is in `extensionsUsed`, and
    /// none is ever in `extensionsRequired`, even when the source required
    /// it.
    #[test]
    fn layers_are_never_required() {
        fn names<'a>(doc: &'a Value, key: &str) -> Vec<&'a str> {
            doc[key].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
        }
        let source = patched(&super::super::fixtures::layered(), |doc| doc["extensionsRequired"] = serde_json::json!(LAYER_EXTENSIONS));
        let read = read_scene(&source, &NoSidecars).unwrap();
        for (fmt, self_contained) in GLTF_FORMS {
            let (doc, _, _) = layer_round_trip(&read.scene, fmt, self_contained);
            let (used, required) = (names(&doc, "extensionsUsed"), names(&doc, "extensionsRequired"));
            for ext in LAYER_EXTENSIONS {
                assert!(used.contains(&ext), "{fmt}: {ext} is not in extensionsUsed {used:?}");
                assert!(!required.contains(&ext), "{fmt}: {ext} is in extensionsRequired {required:?}");
            }
        }
    }

    /// Review Focus 5, glTF's half. KHR_materials_ior's schema allows an `ior`
    /// of 0, a special value, or of 1 and above: `"oneOf": [{ "minimum": 0.0,
    /// "maximum": 0.0 }, { "minimum": 1.0 }]`. Both 0 and the boundary 1
    /// round-trip in GLB and glTF, written as the scene states them.
    #[test]
    fn an_ior_of_zero_round_trips_in_gltf() {
        for ior in [0.0f32, 1.0] {
            let mut scene = super::super::fixtures::layered();
            scene.materials[1].ior = Some(ior);
            for (fmt, self_contained) in GLTF_FORMS {
                let (doc, _, back) = layer_round_trip(&scene, fmt, self_contained);
                assert_eq!(doc["materials"][1]["extensions"]["KHR_materials_ior"], serde_json::json!({ "ior": ior }), "{fmt}: ior {ior}");
                assert_eq!(back.scene.materials[1].ior, Some(ior), "{fmt}: ior {ior}");
            }
        }
    }

    /// A layer is written exactly when the material has it, and each field at
    /// its extension's default is left out, as glTF writers do: a layer at
    /// its defaults is `{}`, and an infinite `attenuationDistance` (nothing
    /// absorbed) is no field at all. `{}` reads back as the layer at its
    /// defaults, so the material keeps every layer it had.
    #[test]
    fn a_layer_at_its_defaults_is_written_as_an_empty_object() {
        let mut scene = super::super::fixtures::layered();
        scene.materials[0] = Material {
            name: Some("defaults".into()),
            clearcoat: Some(Clearcoat::default()),
            transmission: Some(Transmission::default()),
            volume: Some(Volume::default()),
            ior: Some(1.5),
            specular: Some(Specular::default()),
            sheen: Some(Sheen::default()),
            iridescence: Some(Iridescence::default()),
            anisotropy: Some(Anisotropy::default()),
            dispersion: Some(0.0),
            ..Default::default()
        };
        let empty = serde_json::json!({
            "KHR_materials_clearcoat": {},
            "KHR_materials_transmission": {},
            "KHR_materials_volume": {},
            "KHR_materials_ior": {},
            "KHR_materials_specular": {},
            "KHR_materials_sheen": {},
            "KHR_materials_iridescence": {},
            "KHR_materials_anisotropy": {},
            "KHR_materials_dispersion": {}
        });
        for (fmt, self_contained) in GLTF_FORMS {
            let (doc, _, back) = layer_round_trip(&scene, fmt, self_contained);
            assert_eq!(doc["materials"][0]["extensions"], empty, "{fmt}");
            assert_eq!(back.scene.materials[0], scene.materials[0], "{fmt}");
        }
    }

    /// A layer extension whose own JSON does not read (a factor, a
    /// textureInfo, a texture index past `textures`, or an ior
    /// KHR_materials_ior's schema refuses) is dropped with a warning naming
    /// the extension and the field, and the file still converts, as it did
    /// when no layer was read. A layer is optional, so a reader that cannot
    /// use one draws the base layer; base factors still refuse
    /// (`numbers_past_a_32_bit_float_are_refused`). A well-formed layer
    /// beside it is kept, and the dropped one reads none of its textures, so
    /// no texture or image is left behind.
    #[test]
    fn a_malformed_layer_is_dropped_with_a_warning() {
        let cases: [(MaterialLayer, &str, &str, Value); 6] = [
            // A factor of the wrong type, and one past f32.
            (MaterialLayer::Clearcoat, "KHR_materials_clearcoat", "clearcoatFactor", serde_json::json!({ "clearcoatFactor": "1" })),
            (MaterialLayer::Volume, "KHR_materials_volume", "attenuationDistance", serde_json::json!({ "attenuationDistance": 1e39 })),
            // An index past the document's one texture, and a textureInfo with none.
            (MaterialLayer::Sheen, "KHR_materials_sheen", "sheenColorTexture", serde_json::json!({ "sheenColorTexture": { "index": 1 } })),
            (MaterialLayer::Iridescence, "KHR_materials_iridescence", "iridescenceTexture", serde_json::json!({ "iridescenceTexture": { "texCoord": 0 } })),
            // A bad texture after a good one: the good one is not read either.
            (
                MaterialLayer::Clearcoat,
                "KHR_materials_clearcoat",
                "clearcoatRoughnessTexture",
                serde_json::json!({ "clearcoatTexture": { "index": 0 }, "clearcoatRoughnessTexture": { "index": 1 } }),
            ),
            // `validate` would refuse the whole file for it.
            (MaterialLayer::Ior, "KHR_materials_ior", "ior", serde_json::json!({ "ior": 0.5 })),
        ];
        for (layer, ext, field, bad) in cases {
            let r = super::super::read_scene(&one_layer_glb(ext, bad), MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{field}: {e}"));
            assert!(r.warnings.iter().any(|w| w.contains(ext) && w.contains(field)), "{field}: no warning names {ext} and {field}: {:?}", r.warnings);
            let m = &r.scene.materials[0];
            assert!(!m.has_layer(layer), "{field}: the {} layer is kept: {m:?}", layer.as_str());
            assert_eq!(m.transmission, Some(Transmission { factor: 0.5, texture: None }), "{field}: the well-formed layer beside it");
            assert!(r.scene.textures.is_empty() && r.scene.images.is_empty(), "{field}: a texture is left behind");
        }
    }

    /// A layer extension whose value is not a JSON object is its own JSON
    /// failing, as a bad factor is: it is dropped with a warning naming the
    /// extension. Read as the layer at its defaults, it would invent a layer
    /// the file never had, and the writer would emit `{}` for it.
    #[test]
    fn a_layer_that_is_not_an_object_is_dropped_with_a_warning() {
        for (layer, ext, bad) in [
            (MaterialLayer::Clearcoat, "KHR_materials_clearcoat", serde_json::json!(5)),
            (MaterialLayer::Ior, "KHR_materials_ior", Value::Null),
            (MaterialLayer::Sheen, "KHR_materials_sheen", serde_json::json!([])),
            (MaterialLayer::Dispersion, "KHR_materials_dispersion", serde_json::json!("0.3")),
        ] {
            let r = super::super::read_scene(&one_layer_glb(ext, bad.clone()), MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{ext} {bad}: {e}"));
            let m = &r.scene.materials[0];
            assert!(!m.has_layer(layer), "{ext} {bad}: read as a layer: {m:?}");
            assert!(r.warnings.iter().any(|w| w.contains(ext)), "{ext} {bad}: no warning names it: {:?}", r.warnings);
            assert_eq!(m.transmission, Some(Transmission { factor: 0.5, texture: None }), "{ext} {bad}: the well-formed layer beside it");
        }
    }

    /// A value glTF refuses is quoted in the message, cut to 60 characters
    /// and marked "…" past that, and it is never serialized whole to be cut
    /// (Ruling A45): a 100 KB string where a layer's texture index, factor or
    /// whole object belongs comes out cut, in the warning that drops the
    /// layer.
    #[test]
    fn a_hostile_value_in_a_message_is_clipped() {
        let long = "x".repeat(100_000);
        for (ext, value, field) in [
            ("KHR_materials_sheen", serde_json::json!({ "sheenColorTexture": { "index": long } }), "a texture index"),
            ("KHR_materials_sheen", serde_json::json!({ "sheenRoughnessFactor": long }), "a factor"),
            ("KHR_materials_ior", serde_json::json!(long), "the extension"),
        ] {
            let r = super::super::read_scene(&one_layer_glb(ext, value), MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{field}: {e}"));
            let w = r.warnings.iter().find(|w| w.contains(ext)).unwrap_or_else(|| panic!("{field}: no warning names {ext}: {:?}", r.warnings));
            let n = w.chars().count();
            assert!(n < 200 && w.contains("xxx…"), "{field}: {n} characters: {}", w.chars().take(160).collect::<String>());
        }
    }

    /// A triangle GLB whose one material has `value` as its `ext` extension,
    /// beside a well-formed `KHR_materials_transmission`, in a document with
    /// one texture (a 1×1 PNG) that nothing else reads.
    fn one_layer_glb(ext: &str, value: Value) -> Vec<u8> {
        let png = super::super::fixtures::encode_png(1, 1, &[255; 4]);
        let (mut doc, bin) = triangle_doc();
        doc["images"] = serde_json::json!([{ "uri": format!("data:image/png;base64,{}", b64(&png)) }]);
        doc["textures"] = serde_json::json!([{ "source": 0 }]);
        let mut exts = serde_json::Map::new();
        exts.insert(ext.into(), value);
        exts.insert("KHR_materials_transmission".into(), serde_json::json!({ "transmissionFactor": 0.5 }));
        doc["materials"] = serde_json::json!([{ "extensions": exts }]);
        doc["meshes"][0]["primitives"][0]["material"] = Value::from(0);
        doc["extensionsUsed"] = serde_json::json!([ext, "KHR_materials_transmission"]);
        pack(doc, Some(&bin))
    }

    /// The other side of that line (Ruling A16c, revised): a well-formed
    /// layer textureInfo whose texture the document cannot give (a `source`
    /// that is no index, an image or a sampler index out of range) drops that
    /// layer with a warning naming its extension, as a malformed layer is
    /// dropped: before item 4 no layer was read, and such a file converted.
    /// Nothing the layer read before the broken texture is left behind, and
    /// a layer beside it is kept. A broken texture that a base map uses still
    /// refuses the file, as it did before item 4, whether the base map is in
    /// the layer's material or a later one.
    #[test]
    fn a_layer_naming_a_broken_texture_is_dropped_with_a_warning() {
        let png = super::super::fixtures::encode_png(1, 1, &[255; 4]);
        let doc_with = |textures: &Value, materials: Value| {
            let (mut doc, bin) = triangle_doc();
            doc["images"] = serde_json::json!([{ "uri": format!("data:image/png;base64,{}", b64(&png)) }]);
            doc["textures"] = textures.clone();
            let drawn = materials.as_array().map_or(0, Vec::len);
            doc["meshes"][0]["primitives"] = (0..drawn).map(|i| serde_json::json!({ "attributes": { "POSITION": 0 }, "indices": 1, "material": i })).collect();
            doc["materials"] = materials;
            doc["extensionsUsed"] = serde_json::json!(["KHR_materials_sheen", "KHR_materials_clearcoat", "KHR_materials_transmission"]);
            pack(doc, Some(&bin))
        };
        let sheen = serde_json::json!({
            "KHR_materials_sheen": { "sheenColorFactor": [1, 1, 1], "sheenColorTexture": { "index": 0 } },
            "KHR_materials_transmission": { "transmissionFactor": 0.5 }
        });
        let broken = [
            (serde_json::json!([{ "source": "x" }]), "texture 0 source"),
            (serde_json::json!([{ "source": 3 }]), "image index 3 out of range"),
            (serde_json::json!([{ "source": 0, "sampler": 2 }]), "sampler index 2 out of range"),
        ];
        for (textures, detail) in &broken {
            let r = read_scene(&doc_with(textures, serde_json::json!([{ "extensions": sheen }])), &NoSidecars);
            assert!(r.is_ok(), "{textures}: refused: {:?}", r.err());
            let r = r.unwrap();
            let m = &r.scene.materials[0];
            assert_eq!(m.sheen, None, "{textures}");
            assert_eq!(m.transmission, Some(Transmission { factor: 0.5, texture: None }), "{textures}: the layer beside it");
            assert!(r.warnings.iter().any(|w| w.contains("KHR_materials_sheen") && w.contains(detail)), "{textures}: {:?}", r.warnings);
            assert!(r.scene.textures.is_empty() && r.scene.images.is_empty(), "{textures}: left behind: {:?}, {} image(s)", r.scene.textures, r.scene.images.len());
        }
        // A clearcoat whose first map reads and whose second does not goes
        // whole, and leaves the first map's texture behind no more.
        let coat = serde_json::json!([{ "extensions": { "KHR_materials_clearcoat": {
            "clearcoatFactor": 1, "clearcoatTexture": { "index": 0 }, "clearcoatRoughnessTexture": { "index": 1 }
        } } }]);
        let r = read_scene(&doc_with(&serde_json::json!([{ "source": 0 }, { "source": "x" }]), coat), &NoSidecars);
        assert!(r.is_ok(), "the clearcoat: refused: {:?}", r.err());
        let r = r.unwrap();
        assert_eq!(r.scene.materials[0].clearcoat, None);
        assert!(r.warnings.iter().any(|w| w.contains("KHR_materials_clearcoat") && w.contains("texture 1 source")), "{:?}", r.warnings);
        assert!(r.scene.textures.is_empty() && r.scene.images.is_empty(), "the clearcoat's first map is left behind: {:?}", r.scene.textures);
        // A base map on the broken texture refuses, as before item 4.
        let base = serde_json::json!({ "baseColorTexture": { "index": 0 } });
        for (textures, detail) in &broken {
            for materials in [
                serde_json::json!([{ "pbrMetallicRoughness": base, "extensions": sheen }]),
                serde_json::json!([{ "extensions": sheen }, { "pbrMetallicRoughness": base }]),
            ] {
                let r = read_scene(&doc_with(textures, materials.clone()), &NoSidecars).map(|r| r.warnings);
                let d = unwrap_malformed(r.expect_err(&format!("{textures}, {materials}: read")));
                assert!(d.contains(detail), "{textures}, {materials}: {d}");
            }
        }
    }

    /// Ruling B15b's test (mesh-gaps item 2's final wave, F-B15b): a
    /// layer-only texture whose image sits in a meshopt view whose stream
    /// doesn't decode. The file reads; that layer is dropped with a warning
    /// naming it; the base material and the geometry are what they are
    /// without the layer. A base map in such a view refuses the file, as
    /// `a_layer_naming_a_broken_texture_is_dropped_with_a_warning` pins for
    /// the other broken textures (Rulings A47, A47a).
    #[test]
    fn a_layer_only_image_in_a_meshopt_view_that_does_not_decode_drops_its_layer() {
        let (plain_doc, mut bin) = lean_cube();
        let plain = read_document(plain_doc.clone(), Some(&bin), &NoSidecars).unwrap_or_else(|e| panic!("the lean cube: {e}"));
        let mut doc = plain_doc;
        // An ATTRIBUTES stream of two bytes, which ends before its four
        // elements: the decoder's Malformed, not a budget refusal.
        pad_to_4(&mut bin, 0);
        let at = bin.len();
        bin.extend_from_slice(&[0xa0, 0x00]);
        let view = doc["bufferViews"].as_array().unwrap().len();
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({
            "buffer": 1, "byteOffset": 0, "byteLength": 16,
            "extensions": {"KHR_meshopt_compression": {"buffer": 0, "byteOffset": at, "byteLength": 2, "byteStride": 4, "count": 4, "mode": "ATTRIBUTES"}}
        }));
        doc["images"] = serde_json::json!([{"bufferView": view, "mimeType": "image/png"}]);
        doc["textures"] = serde_json::json!([{"source": 0}]);
        doc["materials"] = serde_json::json!([{"extensions": {"KHR_materials_clearcoat": {"clearcoatFactor": 1.0, "clearcoatTexture": {"index": 0}}}}]);
        doc["meshes"][0]["primitives"][0]["material"] = serde_json::json!(0);
        doc["extensionsUsed"].as_array_mut().unwrap().push(serde_json::json!("KHR_materials_clearcoat"));
        let r = read_document(doc, Some(&bin), &NoSidecars).unwrap_or_else(|e| panic!("the file reads: {e}"));
        assert!(
            r.warnings.iter().any(|w| w.contains("KHR_materials_clearcoat") && w.contains("the layer was dropped")),
            "{:?}",
            r.warnings
        );
        assert_eq!(r.scene.materials.len(), 1);
        assert!(r.scene.materials[0].clearcoat.is_none(), "the layer went");
        assert_eq!(r.scene.materials[0].base_color, Material::default().base_color, "the base material stands");
        assert!(r.scene.textures.is_empty() && r.scene.images.is_empty(), "nothing of the dropped layer's texture is left");
        let mut meshes = r.scene.meshes.clone();
        for p in meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
            p.material = None;
        }
        assert!(meshes == plain.scene.meshes, "the geometry is the lean cube's");
    }

    /// 2026-10-12 review, finding 27: properties that change nothing about
    /// the geometry, spelled outside the spec, failed every conversion —
    /// even glb → stl. Each is now read as the default it stands nearest to,
    /// with a warning.
    #[test]
    fn non_geometry_quirks_are_warnings_not_refusals() {
        let quirky = |edit: &dyn Fn(&mut Value)| {
            let (mut doc, bin) = triangle_doc();
            edit(&mut doc);
            read_scene(&pack(doc, Some(&bin)), &NoSidecars)
        };
        let cases: [(&str, &dyn Fn(&mut Value)); 3] = [
            ("alphaMode", &|d| {
                d["materials"] = serde_json::json!([{ "alphaMode": "opaque" }]);
                d["meshes"][0]["primitives"][0]["material"] = Value::from(0);
            }),
            ("interpolation", &|d| {
                let n = d["accessors"].as_array().unwrap().len();
                d["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 0, "componentType": 5126, "count": 1, "type": "SCALAR", "min": [0.0], "max": [0.0]}));
                d["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 0, "componentType": 5126, "count": 1, "type": "VEC3"}));
                d["animations"] = serde_json::json!([{ "samplers": [{ "input": n, "output": n + 1, "interpolation": "linear" }], "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }] }]);
            }),
            ("repeated key time", &|d| {
                let n = d["accessors"].as_array().unwrap().len();
                d["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({"buffer": 0, "byteOffset": 0, "byteLength": 8}));
                let v = d["bufferViews"].as_array().unwrap().len() - 1;
                d["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": v, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [0.0]}));
                d["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 0, "componentType": 5126, "count": 2, "type": "VEC3"}));
                d["animations"] = serde_json::json!([{ "samplers": [{ "input": n, "output": n + 1 }], "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }] }]);
            }),
        ];
        for (what, edit) in cases {
            let r = quirky(edit).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(r.scene.triangle_count(), 1, "{what}");
            assert!(!r.warnings.is_empty(), "{what}: a quirk read without a word");
        }
        // Samplers, on a textured file whose every texture uses the quirky one.
        let pkg = write_gltf_scene(&super::super::fixtures::textured(), true).unwrap();
        let base: Value = serde_json::from_slice(&pkg.files[0].bytes).unwrap();
        for (what, sampler) in [
            ("wrapS", serde_json::json!({ "wrapS": 0 })),
            ("magFilter", serde_json::json!({ "magFilter": 9987 })),
            ("minFilter", serde_json::json!({ "minFilter": 1 })),
        ] {
            let mut doc = base.clone();
            doc["samplers"] = serde_json::json!([sampler]);
            for tex in doc["textures"].as_array_mut().unwrap() {
                tex["sampler"] = Value::from(0);
            }
            let r = read_gltf_scene(&serde_json::to_vec(&doc).unwrap(), &NoSidecars).unwrap_or_else(|e| panic!("{what}: {e}"));
            assert!(r.scene.triangle_count() > 0, "{what}");
            assert!(r.warnings.iter().any(|w| w.contains(what)), "{what}: {:?}", r.warnings);
        }
    }

    #[test]
    fn never_panics_on_single_byte_corruption() {
        let glb = triangle_glb();
        for i in 0..glb.len() {
            for bit in [0x01u8, 0x80] {
                let mut v = glb.clone();
                v[i] ^= bit;
                let _ = read(&v);
            }
        }
    }

    /// `scene` written as `fmt` in one file: a GLB, or a glTF with its buffer
    /// and images embedded.
    fn one_file(scene: &Scene, fmt: MeshFormat) -> Vec<u8> {
        let opts = super::super::WriteOptions { self_contained: true, ..Default::default() };
        super::super::write_scene(scene, fmt, opts).unwrap().package.files[0].bytes.clone()
    }

    /// `file`, a glTF document as `fmt`, swept by [`layer_sweep`] with every
    /// bit of its `materials` array among the single flips: the writer's
    /// keys are sorted, which can put the array past the first 4 KiB. Every
    /// reading is `Ok` or `Err`. No cut reaches a material: a GLB shorter
    /// than its header declares is refused, and a cut JSON document does not
    /// parse.
    fn gltf_materials_sweep(name: &str, file: &[u8], fmt: MeshFormat) {
        layer_sweep(name, file, span(name, file, r#""materials":"#, r#""meshes":"#), |bytes| {
            let _ = super::super::read_scene(bytes, fmt, &NoSidecars);
        });
    }

    /// `scene` as `fmt` in one file ([`one_file`]), read whole, where it must
    /// carry all nine layers and the seven layer maps layered has, then
    /// [`gltf_materials_sweep`].
    fn gltf_layer_sweep(label: &str, scene: &Scene, fmt: MeshFormat) {
        use super::super::scene::TextureSlot;
        let name = &format!("{label} as {}", if fmt == MeshFormat::Glb { "GLB" } else { "one glTF file" });
        let file = one_file(scene, fmt);
        let whole = super::super::read_scene(&file, fmt, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        for layer in MaterialLayer::ALL {
            assert!(whole.scene.materials.iter().any(|m| m.has_layer(layer)), "{name} itself carries {layer:?}");
        }
        let maps = [
            ("car_paint", TextureSlot::Clearcoat),
            ("car_paint", TextureSlot::ClearcoatRoughness),
            ("car_paint", TextureSlot::ClearcoatNormal),
            ("glass", TextureSlot::VolumeThickness),
            ("velvet", TextureSlot::SheenColor),
            ("plastic", TextureSlot::SpecularColor),
            ("soap_film", TextureSlot::IridescenceThickness),
        ];
        for (material, slot) in maps {
            let m = whole.scene.materials.iter().find(|m| m.name.as_deref() == Some(material)).unwrap_or_else(|| panic!("{name} has no {material}"));
            assert!(m.texture_refs().any(|(s, _)| s == slot), "{name}: {material} itself carries its {} map", slot.as_str());
        }
        gltf_materials_sweep(name, &file, fmt);
    }

    /// All seven of layered's materials: each carries glTF layers.
    const LAYERED_MATERIALS: [&str; 7] = ["car_paint", "glass", "velvet", "plastic", "soap_film", "brushed_steel", "lacquer"];

    /// glTF's nine layer extensions through the reader, as a GLB
    /// ([`gltf_layer_sweep`] over all seven of layered's materials, lean:
    /// [`lean_layered`]); and a layer map's `KHR_texture_transform`, which
    /// layered's maps don't carry (Ruling A40c): the lean car paint with its
    /// clearcoat map offset, rotated and scaled, as a GLB, read whole, where
    /// the map must come back with its transform, then
    /// [`gltf_materials_sweep`]. The one-file glTF is
    /// [`gltf_layer_inputs_survive_truncation_and_bit_flips`], split off for
    /// the suite's time (Ruling A40b, as item 11 split its sweep).
    #[test]
    fn glb_layer_inputs_survive_truncation_and_bit_flips() {
        gltf_layer_sweep("lean layered", &lean_layered(&LAYERED_MATERIALS), MeshFormat::Glb);
        let name = "a GLB whose clearcoat map has a KHR_texture_transform";
        let moved = UvTransform { offset: [0.25, 0.5], rotation: 0.5, scale: [2.0, 0.5] };
        let mut coat = lean_layered(&["car_paint"]);
        coat.materials[0].clearcoat.as_mut().and_then(|c| c.texture.as_mut()).expect("the car paint's clearcoat map").transform = Some(moved);
        let file = one_file(&coat, MeshFormat::Glb);
        let info = "/materials/0/extensions/KHR_materials_clearcoat/clearcoatTexture/extensions/KHR_texture_transform";
        assert!(json_of(&file).pointer(info).is_some(), "{name}: its clearcoatTexture carries the extension");
        let whole = super::super::read_scene(&file, MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        let back = whole.scene.materials[0].clearcoat.as_ref().and_then(|c| c.texture.as_ref()).and_then(|t| t.transform);
        assert_eq!(back, Some(moved), "{name}: the layer's textureInfo reads its transform");
        gltf_materials_sweep(name, &file, MeshFormat::Glb);
    }

    /// The one-file glTF half of
    /// [`glb_layer_inputs_survive_truncation_and_bit_flips`]: glTF's nine
    /// layer extensions through the reader ([`gltf_layer_sweep`] over all
    /// seven of layered's materials, lean: [`lean_layered`]), the buffer and
    /// images embedded.
    #[test]
    fn gltf_layer_inputs_survive_truncation_and_bit_flips() {
        gltf_layer_sweep("lean layered", &lean_layered(&LAYERED_MATERIALS), MeshFormat::Gltf);
    }

    /// Both halves' [`gltf_layer_sweep`] over the full-size
    /// `fixtures::layered()`: two minutes in a debug build, seconds released.
    /// Run it with the other full-size sweeps:
    /// `cargo test --release --lib -- --ignored layer_inputs_survive`.
    #[test]
    #[ignore = "full-size layered: minutes in a debug build; run it released (see the doc comment)"]
    fn gltf_layer_inputs_survive_truncation_and_bit_flips_full() {
        let full = super::super::fixtures::layered();
        for fmt in [MeshFormat::Glb, MeshFormat::Gltf] {
            gltf_layer_sweep("layered", &full, fmt);
        }
    }

    /// Minimal encoder, test-only — the module itself never needs to produce
    /// base64.
    fn b64(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for c in bytes.chunks(3) {
            let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
            out.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
        }
        out
    }

    // ── meshopt (mesh-gaps item 2, Task 2) ─────────────────────────────────

    /// The inputs of `testdata/gltf/gltfpack/`: every `fixtures::all()` scene
    /// as this tree's GLB writer writes it, into the folder that
    /// `GLTFPACK_INPUTS_OUT` names. Run by hand when those files are made
    /// again; their README has the gltfpack commands that follow.
    #[test]
    #[ignore = "set GLTFPACK_INPUTS_OUT to a folder; writes the inputs of testdata/gltf/gltfpack/"]
    fn write_the_gltfpack_inputs() {
        let dir = std::env::var("GLTFPACK_INPUTS_OUT").expect("GLTFPACK_INPUTS_OUT names a folder");
        for (name, scene) in super::super::fixtures::all() {
            std::fs::write(format!("{dir}/{name}.glb"), one_file(&scene, MeshFormat::Glb)).unwrap();
        }
    }

    macro_rules! gltfpack_files {
        ($($name:literal),*) => {
            [$(($name, [
                include_bytes!(concat!("testdata/gltf/gltfpack/", $name, ".glb")),
                include_bytes!(concat!("testdata/gltf/gltfpack/", $name, ".c.glb")),
                include_bytes!(concat!("testdata/gltf/gltfpack/", $name, ".cc.glb")),
                include_bytes!(concat!("testdata/gltf/gltfpack/", $name, ".cz.glb")),
            ])),*]
        };
    }

    /// gltfpack 1.3's output of every `fixtures::all()` scene
    /// (`testdata/gltf/gltfpack/`, whose README has the commands): without
    /// `-c`, then with `-c`, `-cc` and `-cz`.
    const GLTFPACK: [(&str, [&[u8]; 4]); 9] =
        gltfpack_files!("cube", "layered", "metal", "morph", "multiuv", "multiuv_shared", "skinned", "staged", "textured");

    fn gltfpack(name: &str) -> [&'static [u8]; 4] {
        GLTFPACK.iter().find(|(n, _)| *n == name).unwrap_or_else(|| panic!("no gltfpack {name}")).1
    }

    // MeshoptCubeTest (glTF-Sample-Assets 8f492390ddc7, CC0; see
    // testdata/gltf/README.md). glTF-Meshopt requires KHR_meshopt_compression,
    // and its fallback buffer has no uri. Its glTF variant only uses the
    // extension, and its fallback buffer is MeshoptCubeTestFallback.bin, the
    // plain data. Both share one stream buffer and one set of images.
    const CUBE: &[u8] = include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/MeshoptCubeTest.gltf");
    const CUBE_BIN: &[u8] = include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/MeshoptCubeTest.bin");
    const CUBE_PLAIN: &[u8] = include_bytes!("testdata/gltf/MeshoptCubeTest/glTF/MeshoptCubeTest.gltf");
    const CUBE_FALLBACK: &[u8] = include_bytes!("testdata/gltf/MeshoptCubeTest/glTF/MeshoptCubeTestFallback.bin");
    /// Every meshopt view of [`CUBE`] as meshoptimizer's JS reference decoder
    /// reads it (`scripts/mesh-verify/meshopt-vectors.mjs --views`): a record
    /// a view, its index and length as u32 little-endian, then the bytes.
    const CUBE_REFERENCE: &[u8] = include_bytes!("testdata/gltf/MeshoptCubeTest/reference-decode.bin");
    const CUBE_IMAGES: [(&str, &[u8]); 10] = [
        ("row0.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/row0.png")),
        ("row1.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/row1.png")),
        ("row2.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/row2.png")),
        ("row3.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/row3.png")),
        ("row4.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/row4.png")),
        ("col0.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/col0.png")),
        ("col1.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/col1.png")),
        ("col2.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/col2.png")),
        ("col3.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/col3.png")),
        ("col4.png", include_bytes!("testdata/gltf/MeshoptCubeTest/glTF-Meshopt/col4.png")),
    ];

    /// The cube's sidecars: its stream buffer and images, and with
    /// `fallback` the glTF variant's fallback buffer.
    fn cube_files(fallback: bool) -> Vec<PackageFile> {
        let mut files: Vec<PackageFile> =
            CUBE_IMAGES.iter().map(|(path, bytes)| PackageFile { path: path.to_string(), bytes: bytes.to_vec() }).collect();
        files.push(PackageFile { path: "MeshoptCubeTest.bin".into(), bytes: CUBE_BIN.to_vec() });
        if fallback {
            files.push(PackageFile { path: "MeshoptCubeTestFallback.bin".into(), bytes: CUBE_FALLBACK.to_vec() });
        }
        files
    }

    fn cube_doc(gltf: &[u8]) -> Value {
        serde_json::from_slice(gltf).unwrap()
    }

    /// `doc` with KHR_meshopt_compression renamed EXT_meshopt_compression
    /// throughout: the same streams under the other extension.
    fn as_ext(doc: &Value) -> Value {
        serde_json::from_str(&doc.to_string().replace("KHR_meshopt_compression", "EXT_meshopt_compression")).unwrap()
    }

    /// `doc` with `ext` taken off its views and out of `extensionsUsed` and
    /// `extensionsRequired`, so that a reader reads each view's fallback.
    fn without_meshopt(mut doc: Value, ext: &str) -> Value {
        for v in doc["bufferViews"].as_array_mut().unwrap() {
            if let Some(e) = v.get_mut("extensions").and_then(Value::as_object_mut) {
                e.remove(ext);
            }
        }
        for key in ["extensionsUsed", "extensionsRequired"] {
            if let Some(list) = doc.get_mut(key).and_then(Value::as_array_mut) {
                list.retain(|e| e != ext);
            }
        }
        doc
    }

    /// MeshoptCubeTest's bufferView `view`: its extension object and stream.
    fn cube_stream(view: usize) -> (Value, &'static [u8]) {
        let e = cube_doc(CUBE)["bufferViews"][view]["extensions"]["KHR_meshopt_compression"].clone();
        let at = e["byteOffset"].as_u64().unwrap_or(0) as usize;
        let len = e["byteLength"].as_u64().unwrap() as usize;
        (e, &CUBE_BIN[at..at + len])
    }

    /// One non-indexed primitive of `count` positions whose view is
    /// compressed under EXT_meshopt_compression, with `object` as the
    /// extension's JSON: the stream in a `data:` URI buffer, and the view's
    /// own `view_len` bytes in a fallback buffer with no uri.
    fn meshopt_doc(object: Value, stream: &[u8], count: usize, view_len: usize) -> Value {
        serde_json::json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["EXT_meshopt_compression"],
            "extensionsRequired": ["EXT_meshopt_compression"],
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{"bufferView": 0, "componentType": 5126, "count": count, "type": "VEC3"}],
            "bufferViews": [{"buffer": 1, "byteLength": view_len, "extensions": {"EXT_meshopt_compression": object}}],
            "buffers": [
                {"uri": format!("data:application/octet-stream;base64,{}", b64(stream)), "byteLength": stream.len()},
                {"byteLength": view_len, "extensions": {"EXT_meshopt_compression": {"fallback": true}}}
            ]
        })
    }

    /// Ruling B1: a meshopt view's declared size, `count × byteStride`
    /// bytes, is charged to MAX_DECODED_VALUES as values of four bytes before
    /// its stream is decoded, and the charge joins the document's running
    /// total. The stream is a header and two bytes, too short for any count,
    /// so a decoder that ran first would answer Malformed ("ends early"):
    /// only the budget answers Unsupported, naming the view and the ceiling.
    #[test]
    fn declared_sizes_are_budgeted_before_allocating() {
        let stream = [0xa0, 0, 0];
        let refused_by_the_budget = |e: &Option<MeshError>| {
            matches!(e, Some(MeshError::Unsupported(d)) if d.contains("bufferView 0") && d.contains(&MAX_DECODED_VALUES.to_string()))
        };
        // 1,000,000 elements of 256 bytes: 64M values, past the 48M ceiling.
        let huge = serde_json::json!({"buffer": 0, "byteLength": 3, "byteStride": 256, "count": 1_000_000, "mode": "ATTRIBUTES"});
        let doc = meshopt_doc(huge, &stream, 3, 256_000_000);
        let e = read_gltf_scene(&serde_json::to_vec(&doc).unwrap(), &NoSidecars).err();
        assert!(refused_by_the_budget(&e), "{e:?}");
        // 1,000 values: within the ceiling alone, past it on top of what the
        // document has already decoded, and let through at exactly the ceiling.
        let small = serde_json::json!({"buffer": 0, "byteLength": 3, "byteStride": 4, "count": 1000, "mode": "ATTRIBUTES"});
        let g = Gltf::new(meshopt_doc(small.clone(), &stream, 3, 4000), None, &NoSidecars);
        g.budget.charge(MAX_DECODED_VALUES - 999, "what the document read before").unwrap();
        let e = g.view_bytes(0).err();
        assert!(refused_by_the_budget(&e), "{e:?}");
        let g = Gltf::new(meshopt_doc(small, &stream, 3, 4000), None, &NoSidecars);
        g.budget.charge(MAX_DECODED_VALUES - 1000, "what the document read before").unwrap();
        let e = g.view_bytes(0).err();
        assert!(matches!(&e, Some(MeshError::Malformed(d)) if d.contains("ends early")), "the budget lets it through to the decoder: {e:?}");
    }

    /// Decoded indices go through the range check every index does. Two
    /// streams hand-built from Appendix A, which meshoptimizer's JS reference
    /// decoder reads as these indices: INDICES (varints 0, 4 and 24 on
    /// baseline 0, deltas 0, +1 and +6, then the 4-byte tail) as [0, 1, 7],
    /// and TRIANGLES (code 0xfe with data byte 0x00, three new vertices; code
    /// 0x00, the last edge and the next vertex; a code table of zeros) as
    /// [0, 1, 2, 0, 2, 3]. Each indexes past a triangle's three positions.
    /// [0, 1, 2] (varints 0, 4, 4) is the control that reads.
    #[test]
    fn out_of_range_decoded_indices_are_refused() {
        let mut triangles = vec![0xe1, 0xfe, 0x00, 0x00];
        triangles.extend([0u8; 16]);
        let cases: [(&str, Vec<u8>, usize, usize, Option<u32>); 3] = [
            ("INDICES", vec![0xd1, 0x00, 0x04, 0x04, 0, 0, 0, 0], 4, 3, None),
            ("INDICES", vec![0xd1, 0x00, 0x04, 0x18, 0, 0, 0, 0], 4, 3, Some(7)),
            ("TRIANGLES", triangles, 2, 6, Some(3)),
        ];
        for (mode, stream, stride, count, bad) in cases {
            let (mut doc, bin) = triangle_doc();
            doc["extensionsUsed"] = serde_json::json!(["EXT_meshopt_compression"]);
            doc["extensionsRequired"] = serde_json::json!(["EXT_meshopt_compression"]);
            doc["accessors"][1]["componentType"] = Value::from(if stride == 4 { COMP_UNSIGNED_INT } else { COMP_UNSIGNED_SHORT });
            doc["accessors"][1]["count"] = Value::from(count);
            doc["bufferViews"][1]["byteLength"] = Value::from(count * stride);
            doc["bufferViews"][1]["extensions"] = serde_json::json!({"EXT_meshopt_compression": {
                "buffer": 1, "byteLength": stream.len(), "byteStride": stride, "count": count, "mode": mode
            }});
            doc["buffers"].as_array_mut().unwrap().push(serde_json::json!({
                "uri": format!("data:application/octet-stream;base64,{}", b64(&stream)), "byteLength": stream.len()
            }));
            let r = read_scene(&pack(doc, Some(&bin)), &NoSidecars);
            match bad {
                None => assert!(r.as_ref().is_ok_and(|r| r.scene.triangle_count() == 1), "{mode}, in range: {:?}", r.err()),
                Some(bad) => {
                    let e = r.err();
                    let want = format!("index {bad} is out of range for 3 vertices");
                    assert!(matches!(&e, Some(MeshError::Malformed(d)) if d.contains(&want)), "{mode}: {e:?}");
                }
            }
        }
    }

    /// The extension's JSON is checked before its stream is touched (Ruling
    /// B1): the view's byteLength must be count × byteStride, the stream's
    /// range must lie in its buffer, the required fields must be there and
    /// read as indices, mode and filter must be names the spec knows, and a
    /// view carries one meshopt extension, not both. Each case is Malformed
    /// and names what failed. The stream is MeshoptCubeTest's bufferView 25
    /// (24 positions, byteStride 12), so the unedited document reads.
    #[test]
    fn a_meshopt_view_that_does_not_describe_itself_is_malformed() {
        let (_, stream) = cube_stream(25);
        let base = serde_json::json!({"buffer": 0, "byteLength": stream.len(), "byteStride": 12, "count": 24, "mode": "ATTRIBUTES"});
        let read = |doc: &Value| read_gltf_scene(&serde_json::to_vec(doc).unwrap(), &NoSidecars);
        let whole = read(&meshopt_doc(base.clone(), stream, 24, 288));
        assert!(whole.as_ref().is_ok_and(|r| r.scene.triangle_count() == 8), "the unedited document reads: {:?}", whole.err());
        let ext = "/bufferViews/0/extensions/EXT_meshopt_compression";
        let set = |field: &'static str, v: Value| -> Box<dyn Fn(&mut Value)> {
            Box::new(move |d: &mut Value| d.pointer_mut(ext).unwrap()[field] = v.clone())
        };
        let without = |field: &'static str| -> Box<dyn Fn(&mut Value)> {
            Box::new(move |d: &mut Value| {
                d.pointer_mut(ext).unwrap().as_object_mut().unwrap().remove(field);
            })
        };
        let cases: Vec<(&str, &str, Box<dyn Fn(&mut Value)>)> = vec![
            ("a view byteLength that isn't count × byteStride", "byteLength", Box::new(|d: &mut Value| d["bufferViews"][0]["byteLength"] = Value::from(287))),
            ("a stream running past its buffer", "spans", set("byteLength", Value::from(stream.len() + 1))),
            ("a stream starting past its buffer's last byte", "spans", set("byteOffset", Value::from(1))),
            ("no count", "count", without("count")),
            ("no byteStride", "byteStride", without("byteStride")),
            ("no byteLength", "byteLength", without("byteLength")),
            ("no buffer", "buffer", without("buffer")),
            ("a buffer that isn't there", "buffer", set("buffer", Value::from(7))),
            ("a count that isn't an index", "count", set("count", Value::from(24.5))),
            ("no mode", "mode", without("mode")),
            ("a mode the spec doesn't name", "mode", set("mode", Value::from("VERTICES"))),
            ("a filter the spec doesn't name", "filter", set("filter", Value::from("SMOOTH"))),
            ("an extension that isn't an object", "buffer", Box::new(|d: &mut Value| *d.pointer_mut(ext).unwrap() = Value::from(true))),
            ("both meshopt extensions", "both", Box::new(|d: &mut Value| {
                let e = d.pointer(ext).unwrap().clone();
                d["bufferViews"][0]["extensions"]["KHR_meshopt_compression"] = e;
            })),
        ];
        for (what, named, edit) in cases {
            let mut doc = meshopt_doc(base.clone(), stream, 24, 288);
            edit(&mut doc);
            let e = read(&doc).err();
            assert!(matches!(&e, Some(MeshError::Malformed(d)) if d.contains(named)), "{what}: {e:?}");
        }
    }

    /// `"fallback": true` with no uri, and a fallback file the caller never
    /// supplied, both read: a meshopt view is decoded from its stream, and
    /// its fallback is never fetched. MeshoptCubeTest's glTF-Meshopt variant
    /// has the first, as a .gltf and packed as a GLB; its glTF variant,
    /// without MeshoptCubeTestFallback.bin, has the second. Each reads its
    /// 320 triangles, and all three read the same scene.
    #[test]
    fn a_meshopt_view_with_a_missing_fallback_reads() {
        let files = cube_files(false);
        let side = package::MemorySidecars { files: &files };
        let mut glb = cube_doc(CUBE);
        glb["buffers"][0].as_object_mut().unwrap().remove("uri");
        let reads = [
            ("glTF-Meshopt", read_gltf_scene(CUBE, &side)),
            ("glTF-Meshopt as a GLB", read_scene(&pack(glb, Some(CUBE_BIN)), &side)),
            ("glTF without MeshoptCubeTestFallback.bin", read_gltf_scene(CUBE_PLAIN, &side)),
        ];
        let mut scenes = Vec::new();
        for (what, r) in reads {
            let r = r.unwrap_or_else(|e| panic!("{what}: {e}"));
            assert_eq!(r.scene.triangle_count(), 320, "{what}");
            scenes.push(r.scene);
        }
        assert!(scenes[1] == scenes[0] && scenes[2] == scenes[0], "the three read the same scene");
    }

    /// Ruling B22 from the other side (Task 5's review, minor 7; F-B22): a
    /// `.gltf` whose meshopt views are only *used*, supplied with its
    /// fallback `MeshoptCubeTestFallback.bin` but not its stream buffer
    /// `MeshoptCubeTest.bin`, is refused as incomplete, naming the missing
    /// buffer under the view and extension it was read for (final review,
    /// Minor 10). The document is cut to one meshopt cube (mesh 15), so the
    /// first read is a stream's and not a plain view's in the same buffer.
    ///
    /// The alternative the final review gives (Ruling B22a): read the
    /// fallback when the stream's buffer is unsupplied and the fallback's is
    /// supplied, which the spec gives the fallback for. It wasn't taken: the
    /// stream is what every such file carries, and the fallback is the one
    /// range a careless or hostile file fills with anything (a GLB's BIN
    /// placeholder, zeros), so a reader that falls back guesses which side
    /// is true. The cost is a rare refusal after the vendor has billed.
    #[test]
    fn a_used_only_meshopt_view_given_its_fallback_but_not_its_stream_is_refused() {
        let mut doc = cube_doc(CUBE_PLAIN);
        let cube = doc["meshes"][15].clone();
        doc["meshes"] = serde_json::json!([cube]);
        doc["nodes"] = serde_json::json!([{"mesh": 0}]);
        doc["scenes"] = serde_json::json!([{"nodes": [0]}]);
        doc.as_object_mut().unwrap().remove("animations");
        doc.as_object_mut().unwrap().remove("skins");
        let whole = read_document(doc.clone(), None, &package::MemorySidecars { files: &cube_files(true) }).unwrap_or_else(|e| panic!("every file supplied: {e}"));
        assert_eq!(whole.scene.triangle_count(), 12, "one cube");
        let files: Vec<PackageFile> = cube_files(true).into_iter().filter(|f| f.path != "MeshoptCubeTest.bin").collect();
        assert!(files.iter().any(|f| f.path == "MeshoptCubeTestFallback.bin"), "the fallback is supplied");
        let e = read_document(doc, None, &package::MemorySidecars { files: &files }).err().expect("refused");
        let d = match e {
            MeshError::Unsupported(d) => d,
            other => panic!("{other:?}"),
        };
        assert!(d.contains("MeshoptCubeTest.bin") && d.contains("not supplied"), "{d}");
        assert!(d.starts_with("bufferView ") && d.contains("KHR_meshopt_compression:"), "named under the view and extension it was read for: {d}");
    }

    /// Final review Minor 1 (Task 2 minor 3): an ATTRIBUTES view whose own
    /// `byteStride` differs from the extension's is Malformed, naming both.
    /// EXT states the equality; a mismatch describes nothing coherent (the
    /// stream decodes `count × byteStride` of the extension's, and the
    /// accessors would step through it at the view's), and this reader
    /// refuses rather than guesses. A view that states the same stride
    /// twice reads.
    #[test]
    fn a_meshopt_view_whose_own_byte_stride_differs_from_the_extensions_is_refused() {
        let (doc, bin) = lean_cube();
        let (index, ext_stride) = doc["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .find_map(|(i, v)| {
                let e = v.pointer("/extensions/KHR_meshopt_compression")?;
                (e["mode"] == "ATTRIBUTES").then(|| (i, e["byteStride"].as_u64().unwrap()))
            })
            .expect("an ATTRIBUTES view");
        let mut same = doc.clone();
        same["bufferViews"][index]["byteStride"] = serde_json::json!(ext_stride);
        read_document(same, Some(&bin), &NoSidecars).unwrap_or_else(|e| panic!("the same stride twice reads: {e}"));
        let mut other = doc;
        other["bufferViews"][index]["byteStride"] = serde_json::json!(ext_stride + 4);
        let d = unwrap_malformed(read_document(other, Some(&bin), &NoSidecars).err().expect("refused"));
        assert!(
            d.contains(&format!("bufferView {index}")) && d.contains(&format!("byteStride is {}", ext_stride + 4)) && d.contains(&format!("byteStride {ext_stride}")),
            "{d}"
        );
    }

    /// Review Focus 1: a meshopt view that several accessors share is
    /// decoded once, and each accessor slices that one buffer from its own
    /// byteOffset. MeshoptCubeTest's interleaved cubes (bufferViews 23, 42,
    /// 61 and 80: a position at 0, a normal at 12 and a colour at 16, byteStride
    /// 20) are each read by three accessors. All three hold the same decoded
    /// bytes, every element is the fallback's, and the budget is charged for
    /// each view once: 480 bytes, 120 values.
    #[test]
    fn one_view_two_accessors_decode_once() {
        let files = cube_files(false);
        let g = Gltf::new(cube_doc(CUBE), None, &package::MemorySidecars { files: &files });
        let plain = cube_doc(CUBE_PLAIN);
        for (view, accessors) in [(23, [25, 26, 27]), (42, [46, 47, 48]), (61, [67, 68, 69]), (80, [88, 89, 90])] {
            let read: Vec<Accessor> = accessors.iter().map(|&a| g.accessor(a).unwrap_or_else(|e| panic!("accessor {a}: {e}"))).collect();
            let fallback_at = plain["bufferViews"][view]["byteOffset"].as_u64().unwrap() as usize;
            for (a, acc) in accessors.iter().zip(&read) {
                assert!(std::ptr::eq(acc.data, read[0].data), "accessor {a} slices bufferView {view}'s one decoded buffer");
                assert_eq!(acc.data.len(), 24 * 20, "accessor {a}: the view's count × byteStride bytes");
                for i in 0..acc.count {
                    let at = fallback_at + acc.start + i * 20;
                    assert_eq!(acc.element(i).unwrap(), &CUBE_FALLBACK[at..at + acc.elem_size], "accessor {a} element {i}");
                }
            }
        }
        assert_eq!(g.budget.values.get(), 4 * 120, "each view charged once");
    }

    /// Review Focus 2, the case the old guard was written for: a GLB whose
    /// meshopt views' own range, their fallback, is the BIN chunk, with the
    /// extension only used. MeshoptCubeTest's glTF variant, packed so: the
    /// BIN chunk is the fallback data, zeroed, then the stream buffer, and
    /// buffer 1 goes. Whether it says KHR_meshopt_compression or
    /// EXT_meshopt_compression, the decoded data wins: the scene is
    /// glTF-Meshopt's, not the zeros'.
    #[test]
    fn a_bin_chunk_fallback_is_never_read_as_geometry() {
        let files = cube_files(false);
        let side = package::MemorySidecars { files: &files };
        let want = read_gltf_scene(CUBE, &side).unwrap_or_else(|e| panic!("glTF-Meshopt: {e}")).scene;
        let mut doc = cube_doc(CUBE_PLAIN);
        assert_eq!(doc["extensionsRequired"], serde_json::json!(["KHR_mesh_quantization"]), "the glTF variant only uses meshopt");
        let shift = CUBE_FALLBACK.len() as u64;
        for v in doc["bufferViews"].as_array_mut().unwrap() {
            let meshopt = v.pointer("/extensions/KHR_meshopt_compression").is_some();
            // A stream moves past the fallback; a meshopt view itself stays
            // on the fallback, now the BIN chunk's first bytes.
            let at = if meshopt { "/extensions/KHR_meshopt_compression/byteOffset" } else { "/byteOffset" };
            let moved = Value::from(v.pointer(at).and_then(Value::as_u64).unwrap_or(0) + shift);
            if meshopt {
                v["extensions"]["KHR_meshopt_compression"]["byteOffset"] = moved;
                v["buffer"] = Value::from(0);
            } else {
                v["byteOffset"] = moved;
            }
        }
        doc["buffers"] = serde_json::json!([{"byteLength": CUBE_FALLBACK.len() + CUBE_BIN.len()}]);
        let mut bin = vec![0u8; CUBE_FALLBACK.len()];
        bin.extend_from_slice(CUBE_BIN);
        for (name, doc) in [("KHR_meshopt_compression", doc.clone()), ("EXT_meshopt_compression", as_ext(&doc))] {
            let got = read_scene(&pack(doc, Some(&bin)), &side).unwrap_or_else(|e| panic!("{name}: {e}")).scene;
            assert!(got == want, "{name}: the BIN chunk's fallback was read as geometry");
        }
    }

    /// litegen's glTF output stays uncompressed: a meshopt file read and
    /// written again as GLB has no meshopt extension in `extensionsUsed` or
    /// `extensionsRequired`, and no compressed view or fallback buffer.
    /// MeshoptCubeTest (KHR) and gltfpack -c's cube (EXT).
    #[test]
    fn meshopt_input_writes_uncompressed() {
        let files = cube_files(false);
        let cube = read_gltf_scene(CUBE, &package::MemorySidecars { files: &files }).unwrap_or_else(|e| panic!("MeshoptCubeTest: {e}"));
        let packed = read_scene(gltfpack("cube")[1], &NoSidecars).unwrap_or_else(|e| panic!("cube.c.glb: {e}"));
        for (name, scene) in [("MeshoptCubeTest", cube.scene), ("cube.c.glb", packed.scene)] {
            let glb = write_scene(&scene).unwrap();
            let doc = json_of(&glb);
            for key in ["extensionsUsed", "extensionsRequired"] {
                let list = doc.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
                assert!(!list.iter().any(|e| e.as_str().is_some_and(|e| e.contains("meshopt"))), "{name}: {key} is {list:?}");
            }
            assert!(!doc.to_string().contains("meshopt"), "{name}: a compressed view or fallback buffer was written");
            let back = read_scene(&glb, &NoSidecars).unwrap_or_else(|e| panic!("{name} written: {e}"));
            assert_eq!(back.scene.triangle_count(), scene.triangle_count(), "{name}");
        }
    }

    /// Ruling B9: EXT_meshopt_compression allows only attribute format v0
    /// (header 0xa0) and has no COLOR filter; KHR adds both. A stream says
    /// which format it is, so an EXT view that breaks either rule still
    /// decodes, and the read names the view and the rule in a warning rather
    /// than refusing a file the vendor has billed for. MeshoptCubeTest
    /// renamed to EXT has 11 v1 views and 6 COLOR views (3 of them both). It
    /// reads as the KHR original, which warns of neither.
    #[test]
    fn an_ext_view_with_a_v1_stream_or_the_color_filter_reads_with_a_warning() {
        let files = cube_files(false);
        let side = package::MemorySidecars { files: &files };
        let khr = read_gltf_scene(CUBE, &side).unwrap_or_else(|e| panic!("KHR: {e}"));
        let ext = read_gltf_scene(&serde_json::to_vec(&as_ext(&cube_doc(CUBE))).unwrap(), &side).unwrap_or_else(|e| panic!("EXT: {e}"));
        assert!(ext.scene == khr.scene, "the EXT file reads as the KHR one");
        // From the streams' header bytes and the views' filters.
        let v1 = [80, 82, 83, 84, 86, 87, 88, 90, 91, 92, 98];
        let color = [65, 69, 73, 84, 88, 92];
        let named = |view: usize, rule: &str| ext.warnings.iter().filter(|w| w.starts_with(&format!("bufferView {view} ")) && w.contains(rule)).count();
        for view in v1 {
            assert_eq!(named(view, "0xa1"), 1, "bufferView {view}: {:?}", ext.warnings);
        }
        for view in color {
            assert_eq!(named(view, "COLOR"), 1, "bufferView {view}: {:?}", ext.warnings);
        }
        let rules = |r: &ReadOutcome| r.warnings.iter().filter(|w| w.contains("only KHR_meshopt_compression")).count();
        assert_eq!(rules(&ext), v1.len() + color.len(), "{:?}", ext.warnings);
        assert_eq!(rules(&khr), 0, "{:?}", khr.warnings);
        // Both extensions are carried: decoded, nothing of them dropped.
        for r in [&khr, &ext] {
            assert!(!r.warnings.iter().any(|w| w.contains("does not carry")), "{:?}", r.warnings);
        }
    }

    /// `got` against `want`, one decoded meshopt view's bytes: byte-equal,
    /// or for OCTAHEDRAL, QUATERNION and COLOR within one unit in the last
    /// place a component, the spec's allowance (OCTAHEDRAL's fourth, which
    /// the filter passes through, exactly). Returns how many components
    /// were a unit off.
    fn within_a_unit(got: &[u8], want: &[u8], filter: &str, stride: usize, ctx: &str) -> usize {
        assert_eq!(got.len(), want.len(), "{ctx}: length");
        let (width, signed) = match (filter, stride) {
            ("OCTAHEDRAL", 4) => (1, true),
            ("COLOR", 4) => (1, false),
            ("OCTAHEDRAL" | "QUATERNION", 8) => (2, true),
            ("COLOR", 8) => (2, false),
            _ => {
                assert!(got == want, "{ctx}: not byte-equal");
                return 0;
            }
        };
        let value = |b: &[u8]| -> i32 {
            match (width, signed) {
                (1, true) => i32::from(b[0] as i8),
                (1, false) => i32::from(b[0]),
                (_, true) => i32::from(i16::from_le_bytes([b[0], b[1]])),
                _ => i32::from(u16::from_le_bytes([b[0], b[1]])),
            }
        };
        let mut off = 0;
        for (i, (g, w)) in got.chunks(width).zip(want.chunks(width)).enumerate() {
            let d = (value(g) - value(w)).abs();
            let allowed = if filter == "OCTAHEDRAL" && i % 4 == 3 { 0 } else { 1 };
            assert!(d <= allowed, "{ctx} component {i}: {} against {}", value(g), value(w));
            off += usize::from(d == 1);
        }
        off
    }

    /// B12, the first part: each of MeshoptCubeTest's 60 meshopt views as
    /// this reader decodes it, against meshoptimizer's JS reference decoder's
    /// reading ([`CUBE_REFERENCE`]): byte-equal, but for a unit in the last
    /// place on OCTAHEDRAL, QUATERNION and COLOR components ([`within_a_unit`];
    /// the reference computes filters in f64, meshopt.rs in f32). The
    /// reference doesn't reach its control-mode-2 bug (Task 1's review, Minor
    /// 7) on these streams.
    #[test]
    fn meshopt_cube_test_decodes_as_the_reference_decoder_does() {
        let files = cube_files(false);
        let doc = cube_doc(CUBE);
        let g = Gltf::new(doc.clone(), None, &package::MemorySidecars { files: &files });
        let (mut views, mut bytes, mut off) = (Vec::new(), 0, 0);
        let mut rest = CUBE_REFERENCE;
        while !rest.is_empty() {
            let view = u32_le(rest, 0).unwrap() as usize;
            let len = u32_le(rest, 4).unwrap() as usize;
            let want = &rest[8..8 + len];
            rest = &rest[8 + len..];
            let e = &doc["bufferViews"][view]["extensions"]["KHR_meshopt_compression"];
            let filter = e["filter"].as_str().unwrap_or("NONE");
            let (got, _) = g.view_bytes(view).unwrap_or_else(|err| panic!("bufferView {view}: {err}"));
            off += within_a_unit(got, want, filter, e["byteStride"].as_u64().unwrap() as usize, &format!("bufferView {view} ({filter})"));
            views.push(view);
            bytes += len;
        }
        let meshopt: Vec<usize> = doc["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, v)| v.pointer("/extensions/KHR_meshopt_compression").is_some())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(views, meshopt, "the reference read every meshopt view, in order");
        eprintln!("MeshoptCubeTest against the JS reference: {} views, {bytes} bytes; {off} filtered components a unit off", views.len());
    }

    /// B12, the second part: each decoded view against its fallback, the
    /// plain data at the view's own byteOffset in MeshoptCubeTestFallback.bin,
    /// within [`within_a_unit`]. The TRIANGLES views hold the same triangles
    /// in the same order, but meshoptimizer's index encoder turns a triangle
    /// to start at another corner when that codes it smaller (winding kept:
    /// `rotateTriangle` in its src/indexcodec.cpp), so each of their
    /// triangles is compared as one of its three turns.
    #[test]
    fn meshopt_cube_test_views_decode_to_their_fallback_data() {
        let files = cube_files(false);
        let doc = cube_doc(CUBE);
        let g = Gltf::new(doc.clone(), None, &package::MemorySidecars { files: &files });
        let (mut equal, mut off, mut triangle_views, mut turned) = (0, 0, 0, 0);
        for (view, v) in doc["bufferViews"].as_array().unwrap().iter().enumerate() {
            let Some(e) = v.pointer("/extensions/KHR_meshopt_compression") else { continue };
            let at = v["byteOffset"].as_u64().unwrap_or(0) as usize;
            let want = &CUBE_FALLBACK[at..at + v["byteLength"].as_u64().unwrap() as usize];
            let (got, _) = g.view_bytes(view).unwrap_or_else(|err| panic!("bufferView {view}: {err}"));
            let stride = e["byteStride"].as_u64().unwrap() as usize;
            if e["mode"] == "TRIANGLES" {
                assert_eq!(got.len(), want.len(), "bufferView {view}: length");
                let index = |b: &[u8], i: usize| if stride == 2 { u32::from(u16::from_le_bytes([b[2 * i], b[2 * i + 1]])) } else { u32_le(b, 4 * i).unwrap() };
                for t in (0..got.len() / stride).step_by(3) {
                    let ours = [index(got, t), index(got, t + 1), index(got, t + 2)];
                    let theirs = [index(want, t), index(want, t + 1), index(want, t + 2)];
                    let turns = [theirs, [theirs[1], theirs[2], theirs[0]], [theirs[2], theirs[0], theirs[1]]];
                    assert!(turns.contains(&ours), "bufferView {view} triangle {}: {ours:?} is no turn of {theirs:?}", t / 3);
                    turned += usize::from(ours != theirs);
                }
                triangle_views += 1;
            } else {
                let o = within_a_unit(got, want, e["filter"].as_str().unwrap_or("NONE"), stride, &format!("bufferView {view}"));
                equal += usize::from(o == 0);
                off += o;
            }
        }
        assert_eq!(equal + triangle_views, 60, "every meshopt view compared");
        eprintln!(
            "MeshoptCubeTest against its fallback: {equal} views byte-equal, {off} filtered components a unit off; \
             {triangle_views} TRIANGLES views equal triangle by triangle, {turned} triangles turned"
        );
    }

    /// How far an attribute decoded through a meshopt filter may sit from
    /// its uncompressed twin's (0 is bit-equal): normals with tangents,
    /// colours and rotation keys absolutely, and translation and scale keys
    /// as a share of their channel's largest magnitude.
    #[derive(Clone, Copy)]
    struct Tolerance {
        normals: f32,
        colors: f32,
        rotations: f32,
        moves: f32,
    }

    /// What [`assert_same_scene`] found: the most each kind of value moved,
    /// and how the twin's vertices and triangles lined up, counted as
    /// vertices at another index, triangles that start at another corner
    /// (winding kept), and triangles at another place.
    #[derive(Debug, Default)]
    struct Drift {
        normals: f32,
        colors: f32,
        rotations: f32,
        moves: f32,
        moved_vertices: usize,
        turned_triangles: usize,
        moved_triangles: usize,
    }

    fn max_diff(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return f32::INFINITY;
        }
        a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
    }

    /// `got`, a meshopt file's scene, against `want`, the same data
    /// uncompressed: everything but geometry and keys equal, and each
    /// primitive holding the same vertices and triangles in any order. A
    /// vertex is matched by what no filter moves (its position, uv sets,
    /// skin influences and morph offsets, bit-equal), then by its normal,
    /// tangent and colour within `tol`. Triangles are compared as sets, each
    /// turned to start at its lowest corner, which keeps its winding.
    fn assert_same_scene(got: &Scene, want: &Scene, tol: Tolerance, ctx: &str) -> Drift {
        assert!(got.nodes == want.nodes && got.roots == want.roots, "{ctx}: the nodes differ");
        assert!(got.materials == want.materials && got.textures == want.textures && got.images == want.images, "{ctx}: the materials differ");
        assert!(got.skins == want.skins && got.lights == want.lights && got.cameras == want.cameras, "{ctx}: the skins, lights or cameras differ");
        assert_eq!(got.meshes.len(), want.meshes.len(), "{ctx}: meshes");
        let mut drift = Drift::default();
        for (mi, (gm, wm)) in got.meshes.iter().zip(&want.meshes).enumerate() {
            assert!(gm.weights == wm.weights && gm.target_names == wm.target_names, "{ctx}: mesh {mi}'s morph weights differ");
            assert_eq!(gm.primitives.len(), wm.primitives.len(), "{ctx}: mesh {mi}'s primitives");
            for (pi, (gp, wp)) in gm.primitives.iter().zip(&wm.primitives).enumerate() {
                same_primitive(gp, wp, tol, &format!("{ctx} mesh {mi} primitive {pi}"), &mut drift);
            }
        }
        assert_eq!(got.animations.len(), want.animations.len(), "{ctx}: animations");
        for (ai, (ga, wa)) in got.animations.iter().zip(&want.animations).enumerate() {
            assert_eq!(ga.channels.len(), wa.channels.len(), "{ctx}: animation {ai}'s channels");
            for (ci, (gc, wc)) in ga.channels.iter().zip(&wa.channels).enumerate() {
                let at = format!("{ctx} animation {ai} channel {ci}");
                assert_eq!((gc.node, gc.path, gc.interpolation), (wc.node, wc.path, wc.interpolation), "{at}");
                assert!(gc.times == wc.times, "{at}: the key times differ");
                let d = max_diff(&gc.values, &wc.values);
                match gc.path {
                    AnimPath::Rotation => {
                        assert!(d <= tol.rotations, "{at}: rotation keys {d} apart");
                        drift.rotations = drift.rotations.max(d);
                    }
                    AnimPath::Translation | AnimPath::Scale => {
                        let largest = wc.values.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                        let share = if largest > 0.0 { d / largest } else { d };
                        assert!(share <= tol.moves, "{at}: {} keys {d} apart, {share} of the largest", gc.path.as_str());
                        drift.moves = drift.moves.max(share);
                    }
                    AnimPath::Weights => assert!(d == 0.0, "{at}: weight keys {d} apart"),
                }
            }
        }
        drift
    }

    /// One primitive of [`assert_same_scene`].
    fn same_primitive(got: &Primitive, want: &Primitive, tol: Tolerance, ctx: &str, drift: &mut Drift) {
        assert_eq!(got.material, want.material, "{ctx}: material");
        let shape = |p: &Primitive| {
            let targets: Vec<bool> = p.targets.iter().map(|t| t.normals.is_some()).collect();
            (p.vertex_count(), p.indices.len(), p.normals.is_some(), p.tangents.is_some(), p.uvs.len(), p.colors.is_some(), p.skin.as_ref().map(|s| s.influences), targets)
        };
        assert_eq!(shape(got), shape(want), "{ctx}: the attributes differ");
        let exact = |p: &Primitive, v: usize| -> Vec<u32> {
            let mut key: Vec<u32> = p.positions[v].iter().map(|f| f.to_bits()).collect();
            for set in &p.uvs {
                key.extend(set[v].iter().map(|f| f.to_bits()));
            }
            if let Some(s) = &p.skin {
                let at = v * s.influences..(v + 1) * s.influences;
                key.extend(s.joints[at.clone()].iter().map(|&j| u32::from(j)));
                key.extend(s.weights[at].iter().map(|f| f.to_bits()));
            }
            for t in &p.targets {
                key.extend(t.positions[v].iter().map(|f| f.to_bits()));
                if let Some(n) = &t.normals {
                    key.extend(n[v].iter().map(|f| f.to_bits()));
                }
            }
            key
        };
        let near = |p: &Primitive, v: usize| -> (Vec<f32>, Vec<f32>) {
            let mut n: Vec<f32> = p.normals.as_ref().map_or(Vec::new(), |n| n[v].to_vec());
            if let Some(t) = &p.tangents {
                n.extend(t[v]);
            }
            (n, p.colors.as_ref().map_or(Vec::new(), |c| c[v].to_vec()))
        };
        let mut twins: HashMap<Vec<u32>, Vec<usize>> = HashMap::new();
        for w in 0..want.vertex_count() {
            twins.entry(exact(want, w)).or_default().push(w);
        }
        let mut taken = vec![false; want.vertex_count()];
        let mut map: Vec<u32> = Vec::with_capacity(got.vertex_count());
        for v in 0..got.vertex_count() {
            let (gn, gc) = near(got, v);
            let best = twins
                .get(&exact(got, v))
                .into_iter()
                .flatten()
                .copied()
                .filter(|&w| !taken[w])
                .map(|w| {
                    let (wn, wc) = near(want, w);
                    (w, max_diff(&gn, &wn), max_diff(&gc, &wc))
                })
                .filter(|&(_, dn, dc)| dn <= tol.normals && dc <= tol.colors)
                .min_by(|a, b| a.1.max(a.2).total_cmp(&b.1.max(b.2)));
            let Some((w, dn, dc)) = best else { panic!("{ctx}: vertex {v} at {:?} has no twin", got.positions[v]) };
            taken[w] = true;
            map.push(w as u32);
            drift.normals = drift.normals.max(dn);
            drift.colors = drift.colors.max(dc);
            drift.moved_vertices += usize::from(w != v);
        }
        let turned = |t: &[u32]| -> [u32; 3] {
            let m = (0..3).min_by_key(|&i| t[i]).unwrap();
            [t[m], t[(m + 1) % 3], t[(m + 2) % 3]]
        };
        let mapped: Vec<[u32; 3]> = got.indices.chunks(3).map(|t| [map[t[0] as usize], map[t[1] as usize], map[t[2] as usize]]).collect();
        for (ours, theirs) in mapped.iter().zip(want.indices.chunks(3)) {
            if ours[..] != *theirs {
                if turned(&ours[..]) == turned(theirs) {
                    drift.turned_triangles += 1;
                } else {
                    drift.moved_triangles += 1;
                }
            }
        }
        let mut ours: Vec<[u32; 3]> = mapped.iter().map(|t| turned(&t[..])).collect();
        let mut theirs: Vec<[u32; 3]> = want.indices.chunks(3).map(turned).collect();
        ours.sort_unstable();
        theirs.sort_unstable();
        assert!(ours == theirs, "{ctx}: the triangles are not the twin's");
    }

    /// B12, the third part: the scene glTF-Meshopt reads as, against the
    /// scene its glTF variant reads as with KHR_meshopt_compression taken
    /// off its views and out of extensionsUsed, so that the reader reads
    /// the fallback ([`assert_same_scene`]). The tolerance is a unit of each
    /// filtered attribute's stored precision at its coarsest: 8-bit normals
    /// and colours, 16-bit rotations.
    #[test]
    fn meshopt_cube_test_reads_as_its_uncompressed_variant() {
        let decoded = read_gltf_scene(CUBE, &package::MemorySidecars { files: &cube_files(false) })
            .unwrap_or_else(|e| panic!("glTF-Meshopt: {e}"))
            .scene;
        let plain = without_meshopt(cube_doc(CUBE_PLAIN), "KHR_meshopt_compression");
        let plain = read_gltf_scene(&serde_json::to_vec(&plain).unwrap(), &package::MemorySidecars { files: &cube_files(true) })
            .unwrap_or_else(|e| panic!("glTF from its fallback: {e}"))
            .scene;
        let tol = Tolerance { normals: 1.0 / 127.0, colors: 1.0 / 255.0, rotations: 1.0 / 32767.0, moves: 0.0 };
        let drift = assert_same_scene(&decoded, &plain, tol, "MeshoptCubeTest");
        assert_eq!((drift.moved_vertices, drift.moved_triangles), (0, 0), "the views keep their order: {drift:?}");
        eprintln!("MeshoptCubeTest against its uncompressed variant: {drift:?}");
    }

    /// The meshopt extensions and filters `glb`'s views use.
    fn meshopt_use(glb: &[u8]) -> (Vec<String>, Vec<String>) {
        let doc = json_of(glb);
        let (mut exts, mut filters) = (Vec::new(), Vec::new());
        for v in doc["bufferViews"].as_array().unwrap() {
            for name in ["EXT_meshopt_compression", "KHR_meshopt_compression"] {
                if let Some(e) = v.pointer(&format!("/extensions/{name}")) {
                    exts.push(name.to_string());
                    filters.push(e["filter"].as_str().unwrap_or("NONE").to_string());
                }
            }
        }
        for list in [&mut exts, &mut filters] {
            list.sort();
            list.dedup();
        }
        (exts, filters)
    }

    /// The filters each gltfpack file is known to carry (Task 2 minor 7),
    /// listed from the files, which the README fixes by sha: -c has NONE
    /// alone; every -cc and -cz file has NONE and OCTAHEDRAL (normals);
    /// skinned adds its animation's EXPONENTIAL and QUATERNION, staged its
    /// animation's QUATERNION; textured's -cz adds COLOR, which only KHR has.
    fn known_filters(name: &str, mode: usize) -> Vec<&'static str> {
        let mut known = if mode == 1 { vec!["NONE"] } else { vec!["NONE", "OCTAHEDRAL"] };
        match (name, mode) {
            ("skinned", 2 | 3) => known.extend(["EXPONENTIAL", "QUATERNION"]),
            ("staged", 2 | 3) => known.push("QUATERNION"),
            ("textured", 3) => known.push("COLOR"),
            _ => {}
        }
        known.sort_unstable();
        known
    }

    /// The first byte of each of `glb`'s ATTRIBUTES streams: the attribute
    /// format's header, 0xa0 (v0) or 0xa1 (v1).
    fn attribute_stream_headers(glb: &[u8]) -> std::collections::BTreeSet<u8> {
        let doc = json_of(glb);
        let bin = 20 + u32_le(glb, 12).unwrap() as usize + 8;
        doc["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| {
                let e = v.pointer("/extensions/EXT_meshopt_compression").or_else(|| v.pointer("/extensions/KHR_meshopt_compression"))?;
                (e["mode"] == "ATTRIBUTES").then(|| glb[bin + e["byteOffset"].as_u64().unwrap_or(0) as usize])
            })
            .collect()
    }

    /// Every fixture's gltfpack file `files[mode]` (1 is -c, 2 -cc, 3 -cz)
    /// against gltfpack's output without -c (`files[0]`), by
    /// [`assert_same_scene`], after checking that it uses `ext`, that the
    /// filters it carries are within `filters` and are exactly the ones it
    /// is known to carry ([`known_filters`]), and that its attribute streams
    /// are v1 (`0xa1`) for -cz and v0 (`0xa0`) otherwise.
    fn gltfpack_twins(mode: usize, ext: &str, filters: &[&str], tol: Tolerance) -> Vec<(String, Drift)> {
        GLTFPACK
            .iter()
            .map(|(name, files)| {
                let ctx = format!("{name}.{}.glb", ["", "c", "cc", "cz"][mode]);
                let (exts, used) = meshopt_use(files[mode]);
                assert_eq!(exts, [ext], "{ctx}");
                assert!(used.iter().all(|f| filters.contains(&f.as_str())), "{ctx}: {used:?}");
                assert_eq!(used, known_filters(name, mode), "{ctx}: the filters the file is known to carry are present");
                let headers = attribute_stream_headers(files[mode]);
                let want = if mode == 3 { 0xa1 } else { 0xa0 };
                assert!(!headers.is_empty() && headers.iter().all(|&h| h == want), "{ctx}: every attribute stream starts 0x{want:02x}: {headers:?}");
                let got = read_scene(files[mode], &NoSidecars).unwrap_or_else(|e| panic!("{ctx}: {e}"));
                let want = read_scene(files[0], &NoSidecars).unwrap_or_else(|e| panic!("{name}.glb: {e}"));
                let drift = assert_same_scene(&got.scene, &want.scene, tol, &ctx);
                (ctx, drift)
            })
            .collect()
    }

    /// gltfpack -c (EXT_meshopt_compression, attribute format v0, no
    /// filter) against gltfpack without -c, for every fixture. B14's one-time
    /// check: gltfpack quantizes alike with and without -c. Every vertex is
    /// bit-equal at the same index and every triangle is in the same place,
    /// but meshoptimizer's index encoder turns some triangles to start at
    /// another corner (winding kept), so the comparison is
    /// [`assert_same_scene`]'s with no tolerance.
    #[test]
    fn gltfpack_c_output_reads_as_its_uncompressed_twin() {
        let exact = Tolerance { normals: 0.0, colors: 0.0, rotations: 0.0, moves: 0.0 };
        for (ctx, drift) in gltfpack_twins(1, "EXT_meshopt_compression", &["NONE"], exact) {
            assert_eq!((drift.moved_vertices, drift.moved_triangles), (0, 0), "{ctx}: {drift:?}");
            eprintln!("{ctx}: {drift:?}");
        }
    }

    /// The filters' precision at gltfpack's default bits, for its -cc and
    /// -cz output against its output without -c. Normals and tangents
    /// (OCTAHEDRAL, -vn 8): 3/127, half a step of the 8-bit octahedral
    /// coordinates, stretched up to twice over the octahedron's folds, and
    /// each side's rounding to its 8-bit output. Rotation keys (QUATERNION,
    /// -ar 12): a 12-bit step, 1/2047. Translation and scale keys
    /// (EXPONENTIAL, -at 16 and -as 16): 2^-14 of the channel's largest
    /// magnitude, since gltfpack rounds those floats to a 16-bit mantissa
    /// without -c and to an exponent shared across the channel with the
    /// filter, so they can't match exactly here. That the filter decodes
    /// exactly is MeshoptCubeTest's six EXPONENTIAL views, byte-equal to the
    /// JS reference's.
    const FILTERED: Tolerance = Tolerance { normals: 3.0 / 127.0, colors: 0.0, rotations: 1.0 / 2047.0, moves: 1.0 / 16384.0 };

    /// gltfpack -cc (EXT_meshopt_compression at a higher level, with the
    /// OCTAHEDRAL, QUATERNION and EXPONENTIAL filters) against gltfpack
    /// without -c, for every fixture, within [`FILTERED`]. -cc also orders
    /// vertices and triangles its own way, which [`assert_same_scene`]
    /// allows.
    #[test]
    fn gltfpack_cc_output_reads_as_its_uncompressed_twin_within_filter_precision() {
        for (ctx, drift) in gltfpack_twins(2, "EXT_meshopt_compression", &["NONE", "OCTAHEDRAL", "QUATERNION", "EXPONENTIAL"], FILTERED) {
            eprintln!("{ctx}: {drift:?}");
        }
    }

    /// gltfpack -cz (KHR_meshopt_compression, attribute format v1, and the
    /// COLOR filter as well) against gltfpack without -c, for every fixture,
    /// within [`FILTERED`] and colours within 1/255 at 8 bits: a unit, and
    /// 1e-6 more for the rounding of the two f32 conversions it is measured
    /// through (a unit apart at 8 bits, two colours read up to 1/255 plus an
    /// f32 rounding apart).
    #[test]
    fn gltfpack_cz_output_reads_as_its_uncompressed_twin_within_filter_precision() {
        let tol = Tolerance { colors: 1.0 / 255.0 + 1e-6, ..FILTERED };
        for (ctx, drift) in gltfpack_twins(3, "KHR_meshopt_compression", &["NONE", "OCTAHEDRAL", "QUATERNION", "EXPONENTIAL", "COLOR"], tol) {
            eprintln!("{ctx}: {drift:?}");
        }
    }

    /// Where `glb`'s meshopt streams lie in the file: from the first byte of
    /// the earliest to the last of the latest, all in its BIN chunk.
    fn stream_span(glb: &[u8]) -> std::ops::Range<usize> {
        let doc = json_of(glb);
        let bin = 20 + u32_le(glb, 12).unwrap() as usize + 8;
        let streams: Vec<(usize, usize)> = doc["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| {
                let e = v.pointer("/extensions/EXT_meshopt_compression").or_else(|| v.pointer("/extensions/KHR_meshopt_compression"))?;
                let at = e["byteOffset"].as_u64().unwrap_or(0) as usize;
                Some((at, at + e["byteLength"].as_u64().unwrap() as usize))
            })
            .collect();
        let start = streams.iter().map(|s| s.0).min().unwrap();
        let end = streams.iter().map(|s| s.1).max().unwrap();
        bin + start..bin + end
    }

    /// MeshoptCubeTest cut down for the in-suite sweep: six of its cubes
    /// (meshes 15, 18, 26, 30, 32 and 33), whose views hold every mode, both
    /// attribute formats, every filter at 8 and 16 bits, both index widths
    /// for INDICES and TRIANGLES, and two views that three accessors share,
    /// with the animation's two QUATERNION outputs (v0 and v1) turning the
    /// first two. Its views' bytes are re-packed into one BIN chunk; the
    /// fallback stays a buffer with no uri. The document and the BIN.
    fn lean_cube() -> (Value, Vec<u8>) {
        let cube = cube_doc(CUBE);
        // Each cube's POSITION, NORMAL, COLOR_0 and indices accessors.
        const CUBES: [[usize; 4]; 6] = [[25, 26, 27, 28], [37, 38, 39, 40], [71, 72, 73, 74], [88, 89, 90, 91], [96, 97, 98, 99], [100, 101, 102, 103]];
        // The two QUATERNION samplers' input and output accessors.
        const SAMPLERS: [[usize; 2]; 2] = [[86, 87], [107, 108]];
        let (mut bin, mut fallback) = (Vec::new(), 0u64);
        let (mut views, mut accessors): (Vec<Value>, Vec<Value>) = (Vec::new(), Vec::new());
        let mut view_of: HashMap<u64, usize> = HashMap::new();
        for a in CUBES.iter().flatten().chain(SAMPLERS.iter().flatten()) {
            let mut accessor = cube["accessors"][*a].clone();
            let old = accessor["bufferView"].as_u64().unwrap();
            if let std::collections::hash_map::Entry::Vacant(slot) = view_of.entry(old) {
                let mut view = cube["bufferViews"][old as usize].clone();
                let compressed = view.pointer("/extensions/KHR_meshopt_compression").is_some();
                let source = if compressed { "/extensions/KHR_meshopt_compression" } else { "" };
                let from = view.pointer(&format!("{source}/byteOffset")).and_then(Value::as_u64).unwrap_or(0) as usize;
                let len = view.pointer(&format!("{source}/byteLength")).and_then(Value::as_u64).unwrap() as usize;
                pad_to_4(&mut bin, 0);
                let at = bin.len();
                bin.extend_from_slice(&CUBE_BIN[from..from + len]);
                if compressed {
                    view["extensions"]["KHR_meshopt_compression"]["byteOffset"] = Value::from(at);
                    view["byteOffset"] = Value::from(fallback);
                    fallback += view["byteLength"].as_u64().unwrap();
                } else {
                    view["byteOffset"] = Value::from(at);
                }
                slot.insert(views.len());
                views.push(view);
            }
            accessor["bufferView"] = Value::from(view_of[&old]);
            accessors.push(accessor);
        }
        let meshes: Vec<Value> = (0..CUBES.len())
            .map(|i| serde_json::json!({"primitives": [{"attributes": {"POSITION": 4 * i, "NORMAL": 4 * i + 1, "COLOR_0": 4 * i + 2}, "indices": 4 * i + 3}]}))
            .collect();
        let nodes: Vec<Value> = (0..CUBES.len()).map(|i| serde_json::json!({"mesh": i, "translation": [2 * i, 0, 0]})).collect();
        let first = 4 * CUBES.len();
        let samplers: Vec<Value> =
            (0..SAMPLERS.len()).map(|s| serde_json::json!({"input": first + 2 * s, "output": first + 2 * s + 1, "interpolation": "LINEAR"})).collect();
        let channels: Vec<Value> = (0..SAMPLERS.len()).map(|s| serde_json::json!({"sampler": s, "target": {"node": s, "path": "rotation"}})).collect();
        let doc = serde_json::json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["KHR_mesh_quantization", "KHR_meshopt_compression"],
            "extensionsRequired": ["KHR_mesh_quantization", "KHR_meshopt_compression"],
            "scenes": [{"nodes": (0..CUBES.len()).collect::<Vec<_>>()}],
            "nodes": nodes,
            "meshes": meshes,
            "animations": [{"samplers": samplers, "channels": channels}],
            "accessors": accessors,
            "bufferViews": views,
            "buffers": [
                {"byteLength": bin.len()},
                {"byteLength": fallback, "extensions": {"KHR_meshopt_compression": {"fallback": true}}}
            ]
        });
        (doc, bin)
    }

    /// The meshopt reader path through the overview's hostile-input bar
    /// ([`layer_sweep`]), on [`lean_cube`]'s BIN chunk, read with its
    /// document parsed once: every prefix, every bit (the chunk is under
    /// 4 KiB), and 10,000 seeded cases. The whole chunk first reads as its
    /// six cubes and two rotation channels. Every reading returns.
    #[test]
    fn meshopt_cube_test_survives_truncation_and_bit_flips() {
        let (doc, bin) = lean_cube();
        assert!(bin.len() <= 4096, "every bit of the {}-byte chunk is flipped", bin.len());
        let whole = read_document(doc.clone(), Some(&bin), &NoSidecars).unwrap_or_else(|e| panic!("lean MeshoptCubeTest: {e}"));
        assert_eq!((whole.scene.triangle_count(), whole.scene.animations[0].channels.len()), (6 * 12, 2), "the lean cube read whole");
        layer_sweep("lean MeshoptCubeTest's BIN chunk", &bin, 0..0, |bytes| {
            let _ = read_document(doc.clone(), Some(bytes), &NoSidecars);
        });
    }

    /// The same bar on all of MeshoptCubeTest's .bin, read with
    /// glTF-Meshopt's .gltf: every prefix of the .bin, every bit of its first
    /// 4 KiB (the plain views and the streams interleaved, row by row: the
    /// first row's plain views sit beside the first streams), and 10,000 seeded
    /// cases across all of it. Two minutes in a debug build; run it
    /// released with the other full-size sweeps:
    /// `cargo test --release --lib -- --ignored survive`.
    #[test]
    #[ignore = "all of MeshoptCubeTest: minutes in a debug build; run it released (see the doc comment)"]
    fn meshopt_cube_test_survives_truncation_and_bit_flips_full() {
        let doc = cube_doc(CUBE);
        let read = |bin: &[u8]| {
            let files = [PackageFile { path: "MeshoptCubeTest.bin".into(), bytes: bin.to_vec() }];
            read_document(doc.clone(), None, &package::MemorySidecars { files: &files })
        };
        let whole = read(CUBE_BIN).unwrap_or_else(|e| panic!("MeshoptCubeTest: {e}"));
        assert_eq!(whole.scene.triangle_count(), 320, "the .bin read whole");
        layer_sweep("MeshoptCubeTest.bin", CUBE_BIN, 0..0, |bytes| {
            let _ = read(bytes);
        });
    }

    /// The same bar on a gltfpack GLB, read whole first: every prefix, every
    /// bit of its first 4 KiB (its JSON) and of its meshopt streams, and
    /// 10,000 seeded cases. Every reading returns.
    fn gltfpack_sweep(name: &str, file: &[u8]) {
        let whole = read_scene(file, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(whole.scene.triangle_count() > 0, "{name} read whole");
        layer_sweep(name, file, stream_span(file), |bytes| {
            let _ = read_scene(bytes, &NoSidecars);
        });
    }

    /// [`gltfpack_sweep`] for -c: staged (EXT_meshopt_compression, v0, no
    /// filter).
    #[test]
    fn gltfpack_c_file_survives_truncation_and_bit_flips() {
        gltfpack_sweep("staged.c.glb", gltfpack("staged")[1]);
    }

    /// [`gltfpack_sweep`] for -cc: staged (EXT_meshopt_compression,
    /// OCTAHEDRAL and QUATERNION).
    #[test]
    fn gltfpack_cc_file_survives_truncation_and_bit_flips() {
        gltfpack_sweep("staged.cc.glb", gltfpack("staged")[2]);
    }

    /// [`gltfpack_sweep`] for -cz: textured (KHR_meshopt_compression, v1,
    /// OCTAHEDRAL and COLOR).
    #[test]
    fn gltfpack_cz_file_survives_truncation_and_bit_flips() {
        gltfpack_sweep("textured.cz.glb", gltfpack("textured")[3]);
    }

    // ── Draco (KHR_draco_mesh_compression, mesh-gaps item 2 Task 4) ─────────

    /// One committable Khronos Draco sample (glTF-Sample-Assets
    /// 8f492390ddc7; testdata/gltf/draco/README.md): its glTF-Draco variant,
    /// which requires the extension and has no fallback, and its plain glTF
    /// variant, each a .gltf and the one .bin it names, with each Draco
    /// stream's quantization in mesh and primitive order. Neither variant's
    /// images were fetched, so both read with their textures dropped and a
    /// warning.
    struct DracoSample {
        name: &'static str,
        draco: [&'static [u8]; 2],
        draco_bin: &'static str,
        plain: [&'static [u8]; 2],
        plain_bin: &'static str,
        streams: &'static [Quantization],
    }

    /// One Draco stream's attribute transforms, as Google's own decoder reads
    /// them (Ruling B31). They follow the attribute values in a stream, so
    /// only a whole decode reads them, and they come from a decoder that
    /// isn't the one under test: three r183's bundled
    /// `examples/jsm/libs/draco/draco_decoder.js` (sha256 `30e4d486…da04`),
    /// with `SkipAttributeTransform` so each transform stays readable. Item
    /// 2's ledger (`.superpowers/sdd/2026-11-04-mesh-compressed-gltf-input/`)
    /// holds the script and its output: `task4/scripts/draco-params.cjs` over
    /// each sample's streams as `task4/scripts/extract_drc.py` cuts them out,
    /// `node task4/scripts/draco-params.cjs <draco_decoder.js as .cjs>
    /// task4/drc/*.drc`, printing `task4/draco-params.jsonl`. Tangents,
    /// joints and weights are stored unquantized in every sample.
    #[derive(Clone, Copy, Debug)]
    struct Quantization {
        /// POSITION's bits, per-axis minimum and range.
        position: (u32, [f64; 3], f64),
        /// TEXCOORD_0's bits and range, where the stream quantizes it.
        uv: Option<(u32, f64)>,
        /// NORMAL's octahedral bits.
        normal: u32,
    }

    impl Quantization {
        /// A quantization step: the range over the top quantized value.
        fn step_of(bits: u32, range: f64) -> f64 {
            range / f64::from((1u32 << bits) - 1)
        }

        fn step(self) -> f64 {
            Self::step_of(self.position.0, self.position.2)
        }

        /// `p` on the position lattice: the quantized value its decode came
        /// from, and the one its original rounds to.
        fn lattice(self, p: [f32; 3]) -> [i64; 3] {
            std::array::from_fn(|i| ((f64::from(p[i]) - self.position.1[i]) / self.step()).round() as i64)
        }

        /// How far a decoded octahedral normal's components may sit from the
        /// original's. Draco rounds the two octahedral coordinates to a
        /// lattice of 2/(2^b − 2) on [−1, 1], so each moves by at most half
        /// that, δ = 1/(2^b − 2). Unfolded, x = 1 − |y| − |z| moves by at most
        /// 2δ (the fold swaps the same magnitudes), so the point on the
        /// octahedron moves by at most √6·δ, and normalizing, from a point of
        /// length at least 1/√3, multiplies that by at most √3: √18·δ per
        /// component, to first order, plus f32 rounding. At 8 bits that is
        /// 0.0167; the worst any sample measures is 0.0128 (WaterBottle).
        fn normal_tolerance(self) -> f32 {
            (18.0f64.sqrt() / f64::from((1u32 << self.normal) - 2)) as f32 + 1e-6
        }
    }

    /// Ruling B31's numbers, per stream (`task4/draco-params.jsonl`).
    const DRACO_SAMPLES: [DracoSample; 5] = [
        DracoSample {
            name: "Box",
            draco: [include_bytes!("testdata/gltf/draco/Box/glTF-Draco/Box.gltf"), include_bytes!("testdata/gltf/draco/Box/glTF-Draco/Box.bin")],
            draco_bin: "Box.bin",
            plain: [include_bytes!("testdata/gltf/draco/Box/glTF/Box.gltf"), include_bytes!("testdata/gltf/draco/Box/glTF/Box0.bin")],
            plain_bin: "Box0.bin",
            streams: &[Quantization { position: (11, [-0.5, -0.5, -0.5], 1.0), uv: None, normal: 8 }],
        },
        DracoSample {
            name: "MorphPrimitivesTest",
            draco: [
                include_bytes!("testdata/gltf/draco/MorphPrimitivesTest/glTF-Draco/MorphPrimitivesTest.gltf"),
                include_bytes!("testdata/gltf/draco/MorphPrimitivesTest/glTF-Draco/MorphPrimitivesTest.bin"),
            ],
            draco_bin: "MorphPrimitivesTest.bin",
            plain: [
                include_bytes!("testdata/gltf/draco/MorphPrimitivesTest/glTF/MorphPrimitivesTest.gltf"),
                include_bytes!("testdata/gltf/draco/MorphPrimitivesTest/glTF/MorphPrimitivesTest.bin"),
            ],
            plain_bin: "MorphPrimitivesTest.bin",
            streams: &[
                Quantization { position: (11, [-0.5, 0.0, -0.5], 1.0), uv: Some((10, 1.0)), normal: 8 },
                Quantization { position: (11, [0.0, 0.0, -0.5], 0.5), uv: Some((10, 0.5)), normal: 8 },
            ],
        },
        DracoSample {
            name: "RiggedSimple",
            draco: [
                include_bytes!("testdata/gltf/draco/RiggedSimple/glTF-Draco/RiggedSimple.gltf"),
                include_bytes!("testdata/gltf/draco/RiggedSimple/glTF-Draco/RiggedSimple0.bin"),
            ],
            draco_bin: "RiggedSimple0.bin",
            plain: [
                include_bytes!("testdata/gltf/draco/RiggedSimple/glTF/RiggedSimple.gltf"),
                include_bytes!("testdata/gltf/draco/RiggedSimple/glTF/RiggedSimple0.bin"),
            ],
            plain_bin: "RiggedSimple0.bin",
            streams: &[Quantization {
                position: (11, [-1.0, -0.999_999_582_767_486_6, -4.575_077_056_884_766], 9.150_154_113_769_531),
                uv: None,
                normal: 8,
            }],
        },
        DracoSample {
            name: "Avocado",
            draco: [include_bytes!("testdata/gltf/draco/Avocado/glTF-Draco/Avocado.gltf"), include_bytes!("testdata/gltf/draco/Avocado/glTF-Draco/Avocado.bin")],
            draco_bin: "Avocado.bin",
            plain: [include_bytes!("testdata/gltf/draco/Avocado/glTF/Avocado.gltf"), include_bytes!("testdata/gltf/draco/Avocado/glTF/Avocado.bin")],
            plain_bin: "Avocado.bin",
            streams: &[Quantization {
                position: (11, [-0.021_280_910_819_768_906, -0.000_047_738_551_074_871_793, -0.013_809_000_141_918_66], 0.062_895_797_193_050_38),
                uv: Some((10, 0.992_390_096_187_591_6)),
                normal: 8,
            }],
        },
        DracoSample {
            name: "WaterBottle",
            draco: [
                include_bytes!("testdata/gltf/draco/WaterBottle/glTF-Draco/WaterBottle.gltf"),
                include_bytes!("testdata/gltf/draco/WaterBottle/glTF-Draco/WaterBottle.bin"),
            ],
            draco_bin: "WaterBottle.bin",
            plain: [include_bytes!("testdata/gltf/draco/WaterBottle/glTF/WaterBottle.gltf"), include_bytes!("testdata/gltf/draco/WaterBottle/glTF/WaterBottle.bin")],
            plain_bin: "WaterBottle.bin",
            streams: &[Quantization {
                position: (11, [-0.054_450_009_018_182_755, -0.130_220_338_702_201_84, -0.054_450_023_919_343_95], 0.260_440_677_404_403_7),
                uv: Some((10, 0.972_773_492_336_273_2)),
                normal: 8,
            }],
        },
    ];

    fn draco_sample(name: &str) -> &'static DracoSample {
        DRACO_SAMPLES.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no Draco sample {name}"))
    }

    /// A .gltf document read with its one .bin supplied under `bin_name`.
    fn read_with_bin(doc: Value, bin_name: &str, bin: &[u8]) -> Result<ReadOutcome, MeshError> {
        let files = [PackageFile { path: bin_name.into(), bytes: bin.to_vec() }];
        read_document(doc, None, &package::MemorySidecars { files: &files })
    }

    /// Box's glTF-Draco document, to edit and read with [`read_box`].
    fn box_draco_doc() -> Value {
        serde_json::from_slice(draco_sample("Box").draco[0]).unwrap()
    }

    fn read_box(doc: Value) -> Result<ReadOutcome, MeshError> {
        read_with_bin(doc, "Box.bin", draco_sample("Box").draco[1])
    }

    /// What [`assert_draco_twins`] measured: the worst of each kind of error
    /// (positions, uvs and morph offsets in quantization steps), the corners
    /// whose vertex sits at another index in the twin, the triangles matched,
    /// and the vertices each side holds (Draco may drop one no triangle uses).
    #[derive(Debug, Default)]
    struct DracoDrift {
        positions: f64,
        uvs: f64,
        normals: f32,
        tangents: f32,
        colors: f32,
        weights: f32,
        targets: f64,
        moved_corners: usize,
        triangles: usize,
        vertices: [usize; 2],
    }

    /// `got`, a Draco variant's scene, against `want`, its plain variant's.
    /// Everything but geometry is equal. In each primitive, Draco may reorder
    /// the vertices, drop one no triangle uses, and start a triangle at
    /// another corner (winding kept), so the triangles are matched as sets of
    /// position triples on the stream's lattice, each turned to start at its
    /// lowest corner. Then each
    /// matched corner's attributes are compared: positions and morph offsets
    /// within a quantization step, uvs within theirs (or `raw` where the
    /// stream doesn't quantize them), normals within
    /// [`Quantization::normal_tolerance`], tangents and colours within `raw`
    /// (raw floats in every sample), joints exactly and weights within 1/255.
    fn assert_draco_twins(got: &Scene, want: &Scene, streams: &[Quantization], raw: f32, ctx: &str) -> DracoDrift {
        assert!(got.nodes == want.nodes && got.roots == want.roots, "{ctx}: the nodes differ");
        assert!(got.materials == want.materials && got.textures == want.textures && got.images == want.images, "{ctx}: the materials differ");
        assert!(got.skins == want.skins && got.lights == want.lights && got.cameras == want.cameras, "{ctx}: the skins, lights or cameras differ");
        assert!(got.animations == want.animations, "{ctx}: the animations differ");
        assert_eq!(got.meshes.len(), want.meshes.len(), "{ctx}: meshes");
        let mut drift = DracoDrift::default();
        let mut streams = streams.iter();
        for (mi, (gm, wm)) in got.meshes.iter().zip(&want.meshes).enumerate() {
            assert!(gm.weights == wm.weights && gm.target_names == wm.target_names, "{ctx}: mesh {mi}'s morph weights differ");
            assert_eq!(gm.primitives.len(), wm.primitives.len(), "{ctx}: mesh {mi}'s primitives");
            for (pi, (gp, wp)) in gm.primitives.iter().zip(&wm.primitives).enumerate() {
                let q = *streams.next().unwrap_or_else(|| panic!("{ctx}: no quantization for mesh {mi} primitive {pi}"));
                same_draco_primitive(gp, wp, q, raw, &format!("{ctx} mesh {mi} primitive {pi}"), &mut drift);
            }
        }
        assert!(streams.next().is_none(), "{ctx}: a stream's quantization was left over");
        drift
    }

    /// One primitive of [`assert_draco_twins`].
    fn same_draco_primitive(got: &Primitive, want: &Primitive, q: Quantization, raw: f32, ctx: &str, drift: &mut DracoDrift) {
        assert_eq!(got.material, want.material, "{ctx}: material");
        let shape = |p: &Primitive| {
            let targets: Vec<bool> = p.targets.iter().map(|t| t.normals.is_some()).collect();
            (p.indices.len(), p.normals.is_some(), p.tangents.is_some(), p.uvs.len(), p.colors.is_some(), p.skin.as_ref().map(|s| s.influences), targets)
        };
        assert_eq!(shape(got), shape(want), "{ctx}: the attributes differ");
        drift.vertices[0] += got.vertex_count();
        drift.vertices[1] += want.vertex_count();
        let turned = |p: &Primitive, t: &[u32]| -> ([[i64; 3]; 3], [usize; 3]) {
            let keys = [q.lattice(p.positions[t[0] as usize]), q.lattice(p.positions[t[1] as usize]), q.lattice(p.positions[t[2] as usize])];
            let m = (0..3).min_by_key(|&i| keys[i]).unwrap();
            let at = |k: usize| (m + k) % 3;
            ([keys[at(0)], keys[at(1)], keys[at(2)]], [t[at(0)] as usize, t[at(1)] as usize, t[at(2)] as usize])
        };
        let mut twins: HashMap<[[i64; 3]; 3], Vec<[usize; 3]>> = HashMap::new();
        for t in want.indices.chunks(3) {
            let (key, corners) = turned(want, t);
            twins.entry(key).or_default().push(corners);
        }
        let step = q.step();
        let uv_step = q.uv.map(|(bits, range)| Quantization::step_of(bits, range));
        // Every error of one corner (a vertex of `got` against one of `want`),
        // or why it is out of tolerance.
        let corner = |v: usize, w: usize| -> Result<DracoDrift, String> {
            let worst = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            let mut d = DracoDrift { moved_corners: usize::from(v != w), ..DracoDrift::default() };
            d.positions = f64::from(worst(&got.positions[v], &want.positions[w])) / step;
            if d.positions > 1.0 {
                return Err(format!("position {:?} is {} steps from {:?}", got.positions[v], d.positions, want.positions[w]));
            }
            if let (Some(gn), Some(wn)) = (&got.normals, &want.normals) {
                d.normals = worst(&gn[v], &wn[w]);
                if d.normals > q.normal_tolerance() {
                    return Err(format!("normal {:?} is {} from {:?}", gn[v], d.normals, wn[w]));
                }
            }
            for (set, (gs, ws)) in got.uvs.iter().zip(&want.uvs).enumerate() {
                let e = worst(&gs[v], &ws[w]);
                let (share, over) = match uv_step {
                    Some(s) if set == 0 => (f64::from(e) / s, f64::from(e) > s),
                    _ => (f64::from(e), e > raw),
                };
                d.uvs = d.uvs.max(share);
                if over {
                    return Err(format!("uv set {set} {:?} is {e} from {:?}", gs[v], ws[w]));
                }
            }
            if let (Some(gt), Some(wt)) = (&got.tangents, &want.tangents) {
                d.tangents = worst(&gt[v], &wt[w]);
                if d.tangents > raw {
                    return Err(format!("tangent {:?} is {} from {:?}", gt[v], d.tangents, wt[w]));
                }
            }
            if let (Some(gc), Some(wc)) = (&got.colors, &want.colors) {
                d.colors = worst(&gc[v], &wc[w]);
                if d.colors > raw {
                    return Err(format!("colour {:?} is {} from {:?}", gc[v], d.colors, wc[w]));
                }
            }
            if let (Some(gs), Some(ws)) = (&got.skin, &want.skin) {
                let ((gj, gw), (wj, ww)) = (gs.vertex(v), ws.vertex(w));
                d.weights = worst(gw, ww);
                if gj != wj || d.weights > 1.0 / 255.0 {
                    return Err(format!("influences {gj:?} {gw:?} against {wj:?} {ww:?}"));
                }
            }
            for (ti, (gt, wt)) in got.targets.iter().zip(&want.targets).enumerate() {
                d.targets = d.targets.max(f64::from(worst(&gt.positions[v], &wt.positions[w])) / step);
                if let (Some(gn), Some(wn)) = (&gt.normals, &wt.normals) {
                    d.normals = d.normals.max(worst(&gn[v], &wn[w]));
                }
                if d.targets > 1.0 {
                    return Err(format!("target {ti}'s offset {:?} is {} steps from {:?}", gt.positions[v], d.targets, wt.positions[w]));
                }
            }
            Ok(d)
        };
        for (ti, t) in got.indices.chunks(3).enumerate() {
            let (key, ours) = turned(got, t);
            let candidates = twins.get_mut(&key).filter(|c| !c.is_empty()).unwrap_or_else(|| {
                panic!("{ctx}: triangle {ti} ({:?}) has no twin with the same lattice corners {key:?}", t.iter().map(|&i| got.positions[i as usize]).collect::<Vec<_>>())
            });
            let mut first_miss = None;
            let found = candidates.iter().position(|theirs| {
                let corners: Result<Vec<DracoDrift>, String> = (0..3).map(|c| corner(ours[c], theirs[c])).collect();
                match corners {
                    Ok(corners) => {
                        for c in corners {
                            drift.positions = drift.positions.max(c.positions);
                            drift.uvs = drift.uvs.max(c.uvs);
                            drift.normals = drift.normals.max(c.normals);
                            drift.tangents = drift.tangents.max(c.tangents);
                            drift.colors = drift.colors.max(c.colors);
                            drift.weights = drift.weights.max(c.weights);
                            drift.targets = drift.targets.max(c.targets);
                            drift.moved_corners += c.moved_corners;
                        }
                        true
                    }
                    Err(why) => {
                        first_miss.get_or_insert(why);
                        false
                    }
                }
            });
            let Some(found) = found else { panic!("{ctx}: triangle {ti}: {}", first_miss.unwrap_or_default()) };
            candidates.swap_remove(found);
            drift.triangles += 1;
        }
    }

    /// `name`'s glTF-Draco variant against its plain glTF variant
    /// ([`assert_draco_twins`]), after checking that the Draco variant
    /// requires the extension and reads with no warning about it. Tangents
    /// and weights are raw floats in each stream: two roundings of one value
    /// to f32, measured at most 3e-8 apart.
    fn draco_twins(name: &str) -> DracoDrift {
        let s = draco_sample(name);
        let doc: Value = serde_json::from_slice(s.draco[0]).unwrap();
        assert!(doc["extensionsRequired"].as_array().is_some_and(|a| a.iter().any(|e| e == "KHR_draco_mesh_compression")), "{name} requires Draco");
        let got = read_with_bin(doc, s.draco_bin, s.draco[1]).unwrap_or_else(|e| panic!("{name} glTF-Draco: {e}"));
        assert!(got.warnings.iter().all(|w| !w.contains("Draco") && !w.contains("draco")), "{name}: {:?}", got.warnings);
        let want = read_with_bin(serde_json::from_slice(s.plain[0]).unwrap(), s.plain_bin, s.plain[1]).unwrap_or_else(|e| panic!("{name} glTF: {e}"));
        let drift = assert_draco_twins(&got.scene, &want.scene, s.streams, 1e-6, name);
        assert_eq!(drift.vertices[0], drift.vertices[1], "{name}: every vertex of the plain variant decodes: {drift:?}");
        eprintln!("{name} against its uncompressed variant: {drift:?}");
        drift
    }

    /// Box: one standard-EdgeBreaker stream, 24 points and 12 faces, its
    /// positions on an 11-bit lattice and its normals octahedral at 8 bits.
    /// Ruling B30's negative controls each fail here.
    #[test]
    fn box_draco_reads_as_its_uncompressed_variant() {
        assert_eq!(draco_twins("Box").triangles, 12);
    }

    /// MorphPrimitivesTest: two sequential streams, the only order-keeping
    /// ones, each primitive with a morph target in plain accessors whose
    /// offsets follow the decoded vertex order.
    #[test]
    fn morph_primitives_test_draco_reads_as_its_uncompressed_variant() {
        let drift = draco_twins("MorphPrimitivesTest");
        assert_eq!((drift.triangles, drift.moved_corners), (32, 0), "sequential streams keep the vertex order: {drift:?}");
    }

    /// RiggedSimple: skinned, standard EdgeBreaker, JOINTS_0 and WEIGHTS_0 in
    /// the stream as generic attributes.
    #[test]
    fn rigged_simple_draco_reads_as_its_uncompressed_variant() {
        assert_eq!(draco_twins("RiggedSimple").triangles, 188);
    }

    /// Avocado: standard EdgeBreaker with TANGENT, a generic attribute.
    #[test]
    fn avocado_draco_reads_as_its_uncompressed_variant() {
        assert_eq!(draco_twins("Avocado").triangles, 682);
    }

    /// WaterBottle: valence EdgeBreaker, which 10 of the 16 Khronos Draco
    /// samples use, with TANGENT.
    #[test]
    fn water_bottle_draco_reads_as_its_uncompressed_variant() {
        assert_eq!(draco_twins("WaterBottle").triangles, 4510);
    }

    /// draco-core 2.3.3's own encoder's sequential mesh of 4,000,001 points,
    /// every one at (0.25, 0.5, 0.75), with one face (made once, by item 2's
    /// ledger tool; testdata/gltf/draco/README.md). Its position attribute is
    /// constant, so the stream is 72 bytes whatever its count. Its header's
    /// point count is the varint at bytes 12..16.
    const CONSTANT_STREAM: &[u8] = include_bytes!("testdata/gltf/draco/crafted/constant-4000001.drc");

    /// [`CONSTANT_STREAM`] with its header saying `points` points: the
    /// varint written over in place. From 2^21 points on a sequential mesh's
    /// indices are 32-bit, so the rest of the stream reads as it did.
    fn constant_stream(points: u32) -> Vec<u8> {
        assert!(points >= 1 << 21, "the face's indices stay 32-bit");
        assert_eq!(&CONSTANT_STREAM[..12], b"DRACO\x02\x02\x01\x00\x00\x00\x01", "a sequential 2.2 mesh of one face");
        let mut varint = Vec::new();
        let mut v = points;
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                varint.push(byte);
                break;
            }
            varint.push(byte | 0x80);
        }
        assert_eq!(varint.len(), 4, "the varint keeps its length");
        [&CONSTANT_STREAM[..12], &varint[..], &CONSTANT_STREAM[16..]].concat()
    }

    /// One required-Draco primitive over `stream` (POSITION is its unique id
    /// 0), in a `data:` URI buffer, whose POSITION accessor declares `count`
    /// elements.
    fn draco_doc(stream: &[u8], count: usize) -> Value {
        serde_json::json!({
            "asset": {"version": "2.0"},
            "extensionsUsed": ["KHR_draco_mesh_compression"],
            "extensionsRequired": ["KHR_draco_mesh_compression"],
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0}],
            "meshes": [{"primitives": [{
                "attributes": {"POSITION": 0},
                "indices": 1,
                "extensions": {"KHR_draco_mesh_compression": {"bufferView": 0, "attributes": {"POSITION": 0}}}
            }]}],
            "accessors": [
                {"componentType": 5126, "count": count, "type": "VEC3"},
                {"componentType": 5125, "count": 3, "type": "SCALAR"}
            ],
            "bufferViews": [{"buffer": 0, "byteLength": stream.len()}],
            "buffers": [{"uri": format!("data:application/octet-stream;base64,{}", b64(stream)), "byteLength": stream.len()}]
        })
    }

    /// The same encoder's sequential mesh of 2,000,000 points at the same
    /// constant, with one face: 24,000,000 bytes of positions, exactly
    /// draco.rs's decoded-bytes ceiling, in 62 bytes. Under 2^21 points its
    /// face's indices are varints, so it is a stream of its own rather than a
    /// rewrite of [`CONSTANT_STREAM`] (testdata/gltf/draco/README.md).
    const CONSTANT_2M: &[u8] = include_bytes!("testdata/gltf/draco/crafted/constant-2000000.drc");

    /// The constant stream saying 100,000,000 points: 1.2 GB of positions in
    /// 72 bytes, the shape draco-core's docs measure. The absolute point
    /// limit, MAX_SCENE_VERTICES, refuses it from the header's count, before
    /// anything is sized from it (fix round 1, Ruling B37), and the refusal is
    /// Unsupported, naming the limit and the stream's claim. A constant stream
    /// within every limit reads, every point the constant: 2,000,000 points,
    /// at the decoded-bytes ceiling (Ruling B38).
    #[test]
    fn a_constant_attribute_cannot_expand_past_the_limits() {
        let stream = constant_stream(100_000_000);
        assert_eq!(stream.len(), 72);
        let d = unwrap_unsupported(read_document(draco_doc(&stream, 100_000_000), None, &NoSidecars).err().expect("refused"));
        assert!(d.contains("declares 100000000 points") && d.contains("4000000"), "{d}");
        let r = read_document(draco_doc(CONSTANT_2M, 2_000_000), None, &NoSidecars).unwrap_or_else(|e| panic!("2,000,000 points: {e}"));
        let p = &r.scene.meshes[0].primitives[0];
        assert_eq!((p.vertex_count(), p.indices.as_slice()), (2_000_000, &[0, 1, 2][..]));
        assert!(p.positions.iter().all(|v| *v == [0.25, 0.5, 0.75]), "every point is the constant");
    }

    /// A Draco primitive is held to the scene's caps by what its stream
    /// says, not by what its accessor declares. A sequential stream of
    /// 4,000,001 points is past MAX_SCENE_VERTICES whether the accessor says
    /// so or says 3, refused from the stream's header before it decodes (fix
    /// round 1). And after a 3-vertex triangle, two streams of 2,000,000
    /// points (each at the decoded-bytes ceiling, so no one stream can carry
    /// more) take the merged mesh past it at the second. Each is Unsupported,
    /// naming the cap.
    #[test]
    fn a_draco_primitive_respects_the_scene_caps() {
        assert_eq!(MAX_MERGED_VERTICES, 4_000_000);
        for declared in [4_000_001, 3] {
            let d = unwrap_unsupported(read_document(draco_doc(&constant_stream(4_000_001), declared), None, &NoSidecars).err().expect("refused"));
            assert!(d.contains("declares 4000001 points") && d.contains("4000000"), "declared {declared}: {d}");
        }
        let mut doc = draco_doc(CONSTANT_2M, 2_000_000);
        let draco = doc["meshes"][0]["primitives"][0].clone();
        doc["meshes"][0]["primitives"].as_array_mut().unwrap().push(draco);
        let triangle = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        doc["buffers"].as_array_mut().unwrap().push(serde_json::json!({"uri": format!("data:application/octet-stream;base64,{}", b64(&triangle)), "byteLength": 36}));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({"buffer": 1, "byteLength": 36}));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"}));
        doc["meshes"][0]["primitives"].as_array_mut().unwrap().insert(0, serde_json::json!({"attributes": {"POSITION": 2}}));
        let d = unwrap_unsupported(read_document(doc, None, &NoSidecars).err().expect("refused"));
        assert!(d.contains("passes 4000000 vertices at mesh 0 primitive 2"), "{d}");
    }

    /// draco-core 2.3.3's own encoder's sequential mesh of 500,001 points
    /// whose one attribute is GENERIC, 48 unsigned bytes a point (the constant
    /// 7), with one face: 24,000,048 bytes of attributes in 45 bytes
    /// (testdata/gltf/draco/README.md).
    const WIDE_NARROW: &[u8] = include_bytes!("testdata/gltf/draco/crafted/narrow-u8x48-500001.drc");

    /// glb.rs passes draco-core the decoded-bytes ceiling set to bound the
    /// Draco decoder's memory on the shapes measured, strips and grids,
    /// 24,000,000 (Rulings B38, B48). So a stream of
    /// 24,000,048 bytes of attributes, within the point and face caps and
    /// under the 192,000,000 the scene's own budget allows, is refused as
    /// Unsupported, naming the ceiling and what it bounds.
    #[test]
    fn a_draco_stream_past_the_decoded_bytes_ceiling_is_refused() {
        let d = unwrap_unsupported(read_document(draco_doc(WIDE_NARROW, 500_001), None, &NoSidecars).err().expect("refused"));
        assert!(d.contains("24000000") && d.contains("memory"), "{d}");
    }

    /// Review Focus 3: a Draco primitive whose extension and attributes
    /// disagree is refused with a message, never read with an attribute's
    /// values under another's name or under none. Refused: an attribute the
    /// extension maps but the primitive doesn't list, one the primitive lists
    /// that the extension doesn't map and that has no data of its own, an id
    /// the stream lacks, two names on one id, and an accessor whose
    /// components aren't the stream's. An unmapped attribute with data of its
    /// own is read as usual, as the extension's README requires (:133: "If
    /// additional attributes are defined in primitive's attributes, but not
    /// defined in KHR_draco_mesh_compression's attributes, then the loader
    /// must process the additional attributes as usual"), and its count must
    /// be the decoded points'.
    #[test]
    fn a_draco_attribute_mismatch_is_refused() {
        const EXT: &str = "/meshes/0/primitives/0/extensions/KHR_draco_mesh_compression/attributes";
        fn edit<'v>(doc: &'v mut Value, pointer: &str) -> &'v mut Value {
            doc.pointer_mut(pointer).unwrap()
        }
        // A TEXCOORD_0 of `count` elements in its own data: URI buffer.
        fn own_uvs(doc: &mut Value, count: usize) {
            let uvs = f32s(&(0..count).flat_map(|k| [k as f32 / 24.0, 1.0 - k as f32 / 24.0]).collect::<Vec<f32>>());
            doc["buffers"].as_array_mut().unwrap().push(serde_json::json!({"uri": format!("data:application/octet-stream;base64,{}", b64(&uvs)), "byteLength": uvs.len()}));
            doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({"buffer": 1, "byteLength": uvs.len()}));
            doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 1, "componentType": 5126, "count": count, "type": "VEC2"}));
            doc["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = serde_json::json!(3);
        }
        let refused = |what: &str, change: &dyn Fn(&mut Value), names: &[&str]| {
            let mut doc = box_draco_doc();
            change(&mut doc);
            let m = unwrap_malformed(read_box(doc).err().expect("refused"));
            assert!(names.iter().all(|n| m.contains(n)), "{what}: {m}");
        };
        refused("mapped, not listed", &|d| edit(d, EXT)["TEXCOORD_0"] = serde_json::json!(1), &["TEXCOORD_0"]);
        refused(
            "listed, not mapped, no data",
            &|d| {
                d["accessors"].as_array_mut().unwrap().push(serde_json::json!({"componentType": 5126, "count": 24, "type": "VEC2"}));
                d["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = serde_json::json!(3);
            },
            &["TEXCOORD_0", "accessor 3"],
        );
        refused("an id the stream lacks", &|d| edit(d, EXT)["NORMAL"] = serde_json::json!(7), &["NORMAL", "7"]);
        refused("two names on one id", &|d| edit(d, EXT)["NORMAL"] = serde_json::json!(1), &["NORMAL", "POSITION"]);
        refused("components", &|d| d["accessors"][1]["type"] = serde_json::json!("VEC2"), &["accessor 1", "VEC2"]);
        refused("an unmapped attribute's count", &|d| own_uvs(d, 23), &["23", "24"]);
        let mut doc = box_draco_doc();
        own_uvs(&mut doc, 24);
        let r = read_box(doc).unwrap_or_else(|e| panic!("an unmapped attribute with its own data: {e}"));
        let want: Vec<[f32; 2]> = (0..24).map(|k| [k as f32 / 24.0, 1.0 - k as f32 / 24.0]).collect();
        assert_eq!(r.scene.meshes[0].primitives[0].uvs, [want], "read as usual, element by element");
    }

    /// The extension's own JSON is checked before anything decodes: it must
    /// name its bufferView and have an attributes object whose ids are
    /// indices, and an accessor whose values the stream gives has no sparse
    /// substitution to apply. Each fault is Malformed, naming it.
    #[test]
    fn a_draco_extension_that_does_not_describe_itself_is_malformed() {
        const EXT: &str = "/meshes/0/primitives/0/extensions/KHR_draco_mesh_compression";
        let refused = |what: &str, change: &dyn Fn(&mut Value), names: &[&str]| {
            let mut doc = box_draco_doc();
            change(&mut doc);
            let m = unwrap_malformed(read_box(doc).err().expect("refused"));
            assert!(names.iter().all(|n| m.contains(n)), "{what}: {m}");
        };
        refused("no bufferView", &|d| drop(d.pointer_mut(EXT).unwrap().as_object_mut().unwrap().remove("bufferView")), &["bufferView"]);
        refused("a bufferView past the file's", &|d| d.pointer_mut(EXT).unwrap()["bufferView"] = serde_json::json!(9), &["bufferView", "9"]);
        refused("no attributes", &|d| drop(d.pointer_mut(EXT).unwrap().as_object_mut().unwrap().remove("attributes")), &["attributes"]);
        refused("an id that is no index", &|d| d.pointer_mut(EXT).unwrap()["attributes"]["NORMAL"] = serde_json::json!("zero"), &["NORMAL", "zero"]);
        refused("a sparse accessor", &|d| d["accessors"][1]["sparse"] = serde_json::json!({"count": 1}), &["NORMAL", "sparse"]);
    }

    /// An accessor the stream fills, whose `count` isn't what the stream
    /// decodes, reads the decoded values: the extension says they are what
    /// the accessor describes (README :112), and the read says the file
    /// misstated it.
    #[test]
    fn a_draco_count_the_accessor_misstates_reads_with_a_warning() {
        let mut doc = box_draco_doc();
        doc["accessors"][1]["count"] = serde_json::json!(23);
        let r = read_box(doc).unwrap_or_else(|e| panic!("Box: {e}"));
        assert_eq!(r.scene.meshes[0].primitives[0].normals.as_ref().map(Vec::len), Some(24), "the decoded normals");
        assert!(r.warnings.iter().any(|w| w.contains("NORMAL accessor 1 declares 23") && w.contains("decodes 24")), "{:?}", r.warnings);
    }

    /// The same for `indices` (Task 4 minor 2, F-T4m2): an `indices`
    /// accessor whose `count` isn't the decoded faces × 3 reads the stream's
    /// indices, with the same kind of warning.
    #[test]
    fn a_draco_indices_count_the_accessor_misstates_reads_with_a_warning() {
        let mut doc = box_draco_doc();
        // Box's accessor 0 is its indices: 36 of them, 12 faces.
        doc["accessors"][0]["count"] = serde_json::json!(33);
        let r = read_box(doc).unwrap_or_else(|e| panic!("Box: {e}"));
        assert_eq!(r.scene.meshes[0].primitives[0].triangle_count(), 12, "the decoded faces");
        assert!(r.warnings.iter().any(|w| w.contains("indices accessor 0 declares 33") && w.contains("decodes 36")), "{:?}", r.warnings);
    }

    /// Ruling B49 (final review Minor 2): an attribute the extension maps
    /// but the primitive doesn't list is skipped with a warning when this
    /// reader wouldn't read that semantic (three.js ignores such a mapping),
    /// and refused when it would: with no accessor there's no componentType
    /// or normalized to read it by.
    #[test]
    fn a_draco_mapping_the_primitive_does_not_list_is_skipped_or_refused_by_whether_it_would_be_read() {
        const EXT: &str = "/meshes/0/primitives/0/extensions/KHR_draco_mesh_compression/attributes";
        let mut doc = box_draco_doc();
        doc.pointer_mut(EXT).unwrap()["_LITEGEN_UNREAD"] = serde_json::json!(9);
        let r = read_box(doc).unwrap_or_else(|e| panic!("an unread mapping is skipped: {e}"));
        assert_eq!(r.scene.meshes[0].primitives[0].triangle_count(), 12);
        assert!(
            r.warnings.iter().any(|w| w.contains("_LITEGEN_UNREAD") && w.contains("don't list") && w.contains("skipped")),
            "{:?}",
            r.warnings
        );
        let mut doc = box_draco_doc();
        doc.pointer_mut(EXT).unwrap()["TEXCOORD_0"] = serde_json::json!(9);
        let d = unwrap_malformed(read_box(doc).err().expect("refused"));
        assert!(d.contains("TEXCOORD_0") && d.contains("don't list") && d.contains("componentType"), "{d}");
    }

    /// The final wave's re-review, N1: the B49 check costs attributes plus
    /// mappings, not attributes × mappings, and warns once. 20,000 unlisted
    /// mappings this reader wouldn't read give one warning naming their
    /// count and a few of them, cut short; one it would read among them
    /// still refuses the file.
    #[test]
    fn many_unlisted_draco_mappings_cost_one_pass_and_one_warning() {
        const EXT: &str = "/meshes/0/primitives/0/extensions/KHR_draco_mesh_compression/attributes";
        let mut doc = box_draco_doc();
        for i in 0..20_000 {
            doc.pointer_mut(EXT).unwrap()[format!("_UNREAD_{i}")] = serde_json::json!(9);
        }
        let r = read_box(doc.clone()).unwrap_or_else(|e| panic!("the unread mappings are skipped: {e}"));
        assert_eq!(r.scene.meshes[0].primitives[0].triangle_count(), 12);
        assert_eq!(r.warnings.len(), 1, "one warning for them all: {:?}", r.warnings.iter().take(3).collect::<Vec<_>>());
        let w = &r.warnings[0];
        assert!(w.contains("20000") && w.contains("_UNREAD_") && w.contains("skipped") && w.len() < 400, "{w}");
        doc.pointer_mut(EXT).unwrap()["TEXCOORD_0"] = serde_json::json!(9);
        let d = unwrap_malformed(read_box(doc).err().expect("refused"));
        assert!(d.contains("TEXCOORD_0") && d.contains("componentType"), "{d}");
    }

    /// A Draco attribute's values take the component type its accessor
    /// declares, as Google's decoder hands them to a loader that asks for one:
    /// an integer as a narrower integer where it fits, refused where it
    /// doesn't, and as a float as it is (no stream here marks its integers
    /// normalized); a float never as an integer.
    #[test]
    fn draco_values_take_their_accessors_component_type() {
        // RiggedSimple's JOINTS_0 (accessor 6) holds 16-bit joints 0 and 1,
        // which read the same as bytes; its WEIGHTS_0 (accessor 9) are floats.
        let s = draco_sample("RiggedSimple");
        let rigged: Value = serde_json::from_slice(s.draco[0]).unwrap();
        let read = |doc: Value| read_with_bin(doc, s.draco_bin, s.draco[1]);
        let joints = |r: ReadOutcome| r.scene.meshes[0].primitives[0].skin.as_ref().map(|k| k.joints.clone());
        let mut narrow = rigged.clone();
        narrow["accessors"][6]["componentType"] = serde_json::json!(COMP_UNSIGNED_BYTE);
        let wide = joints(read(rigged.clone()).unwrap_or_else(|e| panic!("RiggedSimple: {e}")));
        assert!(wide.as_ref().is_some_and(|j| j.contains(&1)), "two joints: {wide:?}");
        assert_eq!(joints(read(narrow).unwrap_or_else(|e| panic!("joints as bytes: {e}"))), wide);
        let mut weights = rigged;
        weights["accessors"][9]["componentType"] = serde_json::json!(COMP_UNSIGNED_BYTE);
        weights["accessors"][9]["normalized"] = serde_json::json!(true);
        let m = unwrap_malformed(read(weights).err().expect("refused"));
        assert!(m.contains("WEIGHTS_0") && m.contains("32-bit floats"), "{m}");

        // textured.q.draco's mesh 0: POSITION (accessor 2) holds 16-bit
        // values up to 32767, past a byte; TEXCOORD_0 (accessor 3) holds
        // unsigned 16-bit ones its accessor normalizes, so as FLOAT they are
        // the integers themselves.
        const DRACO: &[u8] = include_bytes!("testdata/gltf/draco/ours/textured.q.draco.glb");
        let doc = json_of(DRACO);
        let json_len = u32_le(DRACO, 12).unwrap() as usize;
        let bin = &DRACO[20 + json_len + 8..];
        let uvs = |doc: Value| read_document(doc, Some(bin), &NoSidecars).map(|r| r.scene.meshes[0].primitives[0].uvs[0].clone());
        let mut bytes = doc.clone();
        bytes["accessors"][2]["componentType"] = serde_json::json!(COMP_BYTE);
        let m = unwrap_malformed(read_document(bytes, Some(bin), &NoSidecars).err().expect("refused"));
        assert!(m.contains("POSITION") && m.contains("componentType 5120"), "{m}");
        let normalized = uvs(doc.clone()).unwrap_or_else(|e| panic!("textured.q.draco: {e}"));
        let mut floats = doc;
        floats["accessors"][3]["componentType"] = serde_json::json!(COMP_FLOAT);
        floats["accessors"][3]["normalized"] = serde_json::json!(false);
        let raw = uvs(floats).unwrap_or_else(|e| panic!("uvs as floats: {e}"));
        let want: Vec<[f32; 2]> = normalized.iter().map(|uv| uv.map(|c| (c * 65535.0).round())).collect();
        assert!(want.iter().flatten().any(|&c| c > 1.0), "integers past 1: {want:?}");
        assert_eq!(raw, want, "the integers themselves");
    }

    /// An attribute the extension maps but this reader never reads (a custom
    /// `_FOO` here) is never decoded or copied (Ruling B37c). Box with `_FOO`
    /// listed and mapped reads as Box does: mapped to an id its stream lacks,
    /// or to NORMAL's own id, either of which decoding `_FOO` would refuse.
    /// Neither warns, as an uncompressed primitive's ignored attribute
    /// doesn't.
    #[test]
    fn a_mapped_attribute_the_reader_does_not_read_is_never_decoded() {
        let plain = read_box(box_draco_doc()).unwrap_or_else(|e| panic!("Box: {e}")).scene;
        for id in [9, 0] {
            let mut doc = box_draco_doc();
            doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"componentType": 5126, "count": 24, "type": "VEC3"}));
            doc["meshes"][0]["primitives"][0]["attributes"]["_FOO"] = serde_json::json!(3);
            doc["meshes"][0]["primitives"][0]["extensions"]["KHR_draco_mesh_compression"]["attributes"]["_FOO"] = serde_json::json!(id);
            let r = read_box(doc).unwrap_or_else(|e| panic!("_FOO as Draco attribute {id}: {e}"));
            assert!(r.scene == plain, "_FOO as Draco attribute {id}: Box's scene");
            assert!(r.warnings.is_empty(), "_FOO as Draco attribute {id}: {:?}", r.warnings);
        }
        let (mut doc, mut bin) = triangle_doc();
        let at = bin.len();
        bin.extend(f32s(&[1.0; 9]));
        doc["bufferViews"].as_array_mut().unwrap().push(serde_json::json!({"buffer": 0, "byteOffset": at, "byteLength": 36}));
        doc["accessors"].as_array_mut().unwrap().push(serde_json::json!({"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3"}));
        doc["meshes"][0]["primitives"][0]["attributes"]["_FOO"] = serde_json::json!(2);
        doc["buffers"][0]["byteLength"] = serde_json::json!(bin.len());
        let r = read_scene(&pack(doc, Some(&bin)), &NoSidecars).unwrap_or_else(|e| panic!("the uncompressed triangle with _FOO: {e}"));
        assert!(r.warnings.is_empty(), "uncompressed: {:?}", r.warnings);
    }

    /// Review Focus 4: a Draco primitive in TRIANGLE_STRIP mode, which the
    /// extension allows (README :115), holds the stream's faces, a triangle
    /// list, so it reads as the same triangles as mode TRIANGLES, never
    /// expanded as a strip a second time. TRIANGLE_FAN, which the extension
    /// doesn't allow, is refused.
    #[test]
    fn a_draco_strip_decodes_as_a_list() {
        let list = read_box(box_draco_doc()).unwrap_or_else(|e| panic!("Box: {e}")).scene;
        let mut doc = box_draco_doc();
        doc["meshes"][0]["primitives"][0]["mode"] = serde_json::json!(5);
        let strip = read_box(doc.clone()).unwrap_or_else(|e| panic!("Box as a strip: {e}")).scene;
        assert_eq!(strip.triangle_count(), 12);
        assert_eq!(strip.meshes[0].primitives[0].indices, list.meshes[0].primitives[0].indices);
        doc["meshes"][0]["primitives"][0]["mode"] = serde_json::json!(6);
        let m = unwrap_malformed(read_box(doc).err().expect("refused"));
        assert!(m.contains("TRIANGLE_FAN"), "{m}");
    }

    /// Review Focus 5. Our `textured` fixture through glTF-Transform 4.5.1
    /// (testdata/gltf/draco/README.md): `quantize` (KHR_mesh_quantization:
    /// SHORT and UNSIGNED_SHORT normalized attributes, UNSIGNED_BYTE
    /// colours), then `draco`, whose streams hold those integers as they
    /// are. The Draco file reads exactly as the quantized one does: each
    /// value is dequantized by its accessor once the stream is decoded. And
    /// the same file with two of its three streams' views meshopt-compressed
    /// by meshoptimizer 1.2.0's encoder (EXT_meshopt_compression required,
    /// the plain bytes nowhere in the file) reads exactly as it does: each
    /// view is meshopt-decoded, then Draco-decoded, then dequantized.
    #[test]
    fn draco_with_quantization_dequantizes_after_decoding() {
        const QUANTIZED: &[u8] = include_bytes!("testdata/gltf/draco/ours/textured.q.glb");
        const DRACO: &[u8] = include_bytes!("testdata/gltf/draco/ours/textured.q.draco.glb");
        const MESHOPT: &[u8] = include_bytes!("testdata/gltf/draco/ours/textured.q.draco.meshopt.glb");
        let requires = |glb: &[u8]| -> Vec<String> {
            json_of(glb)["extensionsRequired"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect()
        };
        assert_eq!(requires(DRACO), ["KHR_draco_mesh_compression", "KHR_mesh_quantization"]);
        assert_eq!(requires(MESHOPT), ["KHR_draco_mesh_compression", "KHR_mesh_quantization", "EXT_meshopt_compression"]);
        let wrapped: Vec<usize> = json_of(MESHOPT)["meshes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.pointer("/primitives/0/extensions/KHR_draco_mesh_compression/bufferView").unwrap().as_u64().unwrap() as usize)
            .filter(|&v| json_of(MESHOPT)["bufferViews"][v].pointer("/extensions/EXT_meshopt_compression").is_some())
            .collect();
        assert_eq!(wrapped, [1, 2], "two of the three streams' views are meshopt");
        let read = |glb: &[u8], what: &str| read_scene(glb, &NoSidecars).unwrap_or_else(|e| panic!("{what}: {e}")).scene;
        let (quantized, draco, meshopt) = (read(QUANTIZED, "quantized"), read(DRACO, "Draco"), read(MESHOPT, "meshopt over Draco"));
        let positions: Vec<f32> = draco.meshes.iter().flat_map(|m| &m.primitives).flat_map(|p| p.positions.iter().flatten().copied()).collect();
        assert!(positions.iter().all(|v| v.abs() <= 1.0) && positions.iter().any(|v| v.abs() == 1.0), "SHORT normalized, dequantized: {positions:?}");
        // The lattice only pairs the triangles here: every value must be exact.
        let grid = [Quantization { position: (16, [-1.0; 3], 2.0), uv: Some((16, 1.0)), normal: 16 }; 3];
        for (got, want, what) in [(&draco, &quantized, "Draco against quantized"), (&meshopt, &draco, "meshopt over Draco against Draco")] {
            let drift = assert_draco_twins(got, want, &grid, 0.0, what);
            let errors = (drift.positions, drift.uvs, drift.normals, drift.tangents, drift.colors, drift.weights, drift.targets);
            assert_eq!(errors, (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0), "{what}: {drift:?}");
            eprintln!("{what}: {drift:?}");
        }
    }

    /// The overview's hostile-input bar on Box: its glTF-Draco .bin (the
    /// stream, and two bytes of padding), then its .gltf, whose extension
    /// objects the reader parses, each cut at every length, every bit
    /// flipped, and 10,000 seeded cases ([`layer_sweep`]). Every reading
    /// returns.
    #[test]
    fn box_draco_survives_truncation_and_bit_flips() {
        let s = draco_sample("Box");
        let whole = read_box(box_draco_doc()).unwrap_or_else(|e| panic!("Box: {e}"));
        assert_eq!(whole.scene.triangle_count(), 12, "Box read whole");
        layer_sweep("Box's glTF-Draco .bin", s.draco[1], 0..0, |bytes| {
            let _ = read_with_bin(box_draco_doc(), "Box.bin", bytes);
        });
        let files = [PackageFile { path: "Box.bin".into(), bytes: s.draco[1].to_vec() }];
        layer_sweep("Box's glTF-Draco .gltf", s.draco[0], 0..0, |bytes| {
            let _ = read_gltf_scene(bytes, &package::MemorySidecars { files: &files });
        });
    }

    /// The same bar on MorphPrimitivesTest's glTF-Draco .bin: its two morph
    /// targets' plain views, then its two sequential streams.
    #[test]
    fn morph_primitives_test_draco_survives_truncation_and_bit_flips() {
        let s = draco_sample("MorphPrimitivesTest");
        let doc: Value = serde_json::from_slice(s.draco[0]).unwrap();
        let whole = read_with_bin(doc.clone(), s.draco_bin, s.draco[1]).unwrap_or_else(|e| panic!("MorphPrimitivesTest: {e}"));
        assert_eq!(whole.scene.triangle_count(), 32, "MorphPrimitivesTest read whole");
        layer_sweep("MorphPrimitivesTest's glTF-Draco .bin", s.draco[1], 0..0, |bytes| {
            let _ = read_with_bin(doc.clone(), s.draco_bin, bytes);
        });
    }

    // ── points and lines (mesh-gaps item 10) ────────────────────────────────

    /// glTF's modes 0 to 3 as the scene's points and lines: read from a
    /// hand-built document, written back as modes 0 and 1, charged to the
    /// index budget at their expanded size, and swept with truncations and bit
    /// flips. Its own module, so that its helpers' names stay its own.
    mod points_and_lines {
        use super::*;
        use super::super::super as mesh;
        use mesh::scene::Topology;

        /// The four corners every primitive of [`modes_glb`] draws from.
        const QUAD: [[f32; 3]; 4] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];

        /// One mesh, one node, and a primitive per `(mode, indices)`: each
        /// draws [`QUAD`] (one POSITION accessor they share) in its `mode`,
        /// with its `indices` as an accessor of its own, or none.
        fn modes_glb(prims: &[(u64, Option<&[u32]>)]) -> Vec<u8> {
            let flat: Vec<f32> = QUAD.iter().flatten().copied().collect();
            let mut bin = f32s(&flat);
            let mut views = vec![serde_json::json!({"buffer": 0, "byteOffset": 0, "byteLength": bin.len()})];
            let mut accessors = vec![serde_json::json!({
                "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]
            })];
            let mut primitives = Vec::new();
            for &(mode, indices) in prims {
                let mut prim = serde_json::json!({"attributes": {"POSITION": 0}, "mode": mode});
                if let Some(ix) = indices {
                    views.push(serde_json::json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": ix.len() * 4}));
                    bin.extend_from_slice(&u32s(ix));
                    accessors.push(serde_json::json!({
                        "bufferView": views.len() - 1, "componentType": 5125, "count": ix.len(), "type": "SCALAR"
                    }));
                    prim["indices"] = Value::from(accessors.len() - 1);
                }
                primitives.push(prim);
            }
            let doc = serde_json::json!({
                "asset": {"version": "2.0"},
                "scenes": [{"nodes": [0]}],
                "nodes": [{"mesh": 0}],
                "meshes": [{"primitives": primitives}],
                "accessors": accessors,
                "bufferViews": views,
                "buffers": [{"byteLength": bin.len()}]
            });
            pack(doc, Some(&bin))
        }

        /// What a reading of `glb` as a GLB gives, or the error it gave.
        fn read_glb(glb: &[u8]) -> Result<ReadOutcome, MeshError> {
            read_scene(glb, &NoSidecars)
        }

        /// The elements `topology` draws in `s`: triangles, segments or points.
        fn elements(s: &Scene, topology: Topology) -> usize {
            s.meshes
                .iter()
                .flat_map(|m| &m.primitives)
                .filter(|p| p.topology == topology)
                .map(|p| p.indices.len() / topology.indices_per_element())
                .sum()
        }

        /// The first primitive of the mesh called `name`.
        fn prim_of<'a>(s: &'a Scene, name: &str) -> &'a Primitive {
            let mesh = s.meshes.iter().find(|m| m.name.as_deref() == Some(name));
            &mesh.unwrap_or_else(|| panic!("no mesh called {name}")).primitives[0]
        }

        /// A Malformed reading's detail, or a panic naming what came instead.
        fn malformed_detail(glb: &[u8]) -> String {
            match read_glb(glb) {
                Err(MeshError::Malformed(d)) => d,
                Err(other) => panic!("expected Malformed, got {other:?}"),
                Ok(r) => panic!("read as {:?}, with warnings {:?}", r.scene.meshes[0].primitives.iter().map(|p| p.topology).collect::<Vec<_>>(), r.warnings),
            }
        }

        // ── reading ─────────────────────────────────────────────────────────

        /// Mode 0 is points, 1 lines, 2 a loop and 3 a strip (glTF 2.0
        /// §3.7.2.1 "Primitive.mode"). A loop of n vertices is n segments (its
        /// closing one included) and a strip n - 1; with no `indices`, a
        /// primitive walks its vertices in order.
        #[test]
        fn glb_reads_points_lines_loops_and_strips() {
            let table: [(&str, u64, Option<&[u32]>, Topology, &[u32]); 8] = [
                ("points", 0, None, Topology::Points, &[0, 1, 2, 3]),
                ("indexed points", 0, Some(&[3, 1]), Topology::Points, &[3, 1]),
                ("lines", 1, None, Topology::Lines, &[0, 1, 2, 3]),
                ("indexed lines", 1, Some(&[0, 2, 1, 3]), Topology::Lines, &[0, 2, 1, 3]),
                ("a loop of 4 vertices", 2, None, Topology::Lines, &[0, 1, 1, 2, 2, 3, 3, 0]),
                ("an indexed loop of 3", 2, Some(&[2, 0, 3]), Topology::Lines, &[2, 0, 0, 3, 3, 2]),
                ("a strip of 4 vertices", 3, None, Topology::Lines, &[0, 1, 1, 2, 2, 3]),
                ("an indexed strip of 3", 3, Some(&[3, 0, 2]), Topology::Lines, &[3, 0, 0, 2]),
            ];
            let glb = modes_glb(&table.map(|(_, mode, indices, _, _)| (mode, indices)));
            let r = read_glb(&glb).unwrap();
            assert!(r.warnings.is_empty(), "nothing was skipped: {:?}", r.warnings);
            let prims = &r.scene.meshes[0].primitives;
            assert_eq!(prims.len(), table.len());
            for (p, (what, _, _, topology, indices)) in prims.iter().zip(table) {
                assert_eq!((p.topology, p.indices.as_slice()), (topology, indices), "{what}");
                assert_eq!(p.positions, QUAD, "{what}");
            }
            r.scene.validate().unwrap();
            // The loop of 4 is 4 segments and the loop of 3 is 3; the strips are 3 and 2.
            assert_eq!((elements(&r.scene, Topology::Triangles), elements(&r.scene, Topology::Lines), elements(&r.scene, Topology::Points)), (0, 4 + 3 + 3 + 2 + 2 + 2, 4 + 2));
            assert_eq!((r.scene.triangle_count(), r.scene.drawable_count()), (0, 22));
        }

        /// A lines primitive whose indices are not whole segments, and a loop
        /// or a strip of one vertex, are refused as a triangle list that is
        /// not whole triangles is.
        #[test]
        fn lines_that_cannot_be_whole_are_refused() {
            let cases: [(&str, u64, &[u32], &str); 3] = [
                ("three indices of a line list", 1, &[0, 1, 2], "not a multiple of 2"),
                ("a strip of one vertex", 3, &[2], "at least 2"),
                ("a loop of one vertex", 2, &[2], "at least 2"),
            ];
            for (what, mode, indices, expected) in cases {
                let d = malformed_detail(&modes_glb(&[(mode, Some(indices))]));
                assert!(d.contains(expected) && d.contains("mesh 0 primitive 0"), "{what}: {d}");
            }
        }

        /// glTF 2.0 defines modes 0 to 6; anything else is skipped with a
        /// warning, as it was before points and lines were read.
        #[test]
        fn a_mode_gltf_does_not_define_is_skipped_with_a_warning() {
            let r = read_glb(&modes_glb(&[(7, None), (0, None)])).unwrap();
            assert_eq!(r.scene.meshes[0].primitives.len(), 1, "only the points primitive is read");
            let w: Vec<&String> = r.warnings.iter().filter(|w| w.contains("mode 7")).collect();
            assert!(w.len() == 1 && w[0].contains("does not define") && w[0].contains("skipped"), "{:?}", r.warnings);
        }

        /// Lines and points keep what a triangle keeps: positions, normals,
        /// tangents, uvs, colours, morph targets and a material, through both
        /// writers and back.
        #[test]
        fn lines_and_points_carry_what_triangles_carry() {
            let mut s = Scene::default();
            s.materials.push(Material { name: Some("ink".into()), base_color: [0.5, 0.25, 0.125, 1.0], ..Default::default() });
            let carrying = |topology: Topology, indices: Vec<u32>| Primitive {
                positions: QUAD.to_vec(),
                normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
                tangents: Some(vec![[1.0, 0.0, 0.0, 1.0]; 4]),
                uvs: vec![vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
                colors: Some(vec![[1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0], [0.5, 0.5, 0.5, 0.5]]),
                targets: vec![MorphTarget { positions: vec![[0.0, 0.0, 0.5]; 4], normals: None }],
                indices,
                material: Some(0),
                topology,
                ..Default::default()
            };
            for (name, prim) in [("lines", carrying(Topology::Lines, vec![0, 1, 2, 3])), ("points", carrying(Topology::Points, vec![3, 0, 1]))] {
                s.meshes.push(SceneMesh { name: Some(name.into()), primitives: vec![prim], weights: vec![0.5], ..Default::default() });
                let mesh = s.meshes.len() - 1;
                s.add_root(Node { name: Some(format!("{name}_node")), mesh: Some(mesh), weights: Some(vec![0.5]), ..Default::default() });
            }
            s.validate().unwrap();
            for (fmt, self_contained) in GLTF_FORMS {
                let (_, losses, back) = layer_round_trip(&s, fmt, self_contained);
                assert!(losses.is_empty(), "{fmt}: {losses:?}");
                for name in ["lines", "points"] {
                    assert_eq!(prim_of(&back.scene, name), prim_of(&s, name), "{fmt}: {name}");
                }
            }
        }

        // ── writing ─────────────────────────────────────────────────────────

        /// `fixtures::wire()` (a cube, a coloured line loop and a coloured
        /// grid of points) through the writer and the reader, as a GLB and as
        /// a glTF in one file and in a package: nothing lost, the triangles
        /// without a mode (as ever) and the loop's and the grid's written as
        /// modes 1 and 0, the same elements of each kind, positions bit for bit
        /// and the same colours.
        #[test]
        fn wire_round_trips_through_glb_and_gltf() {
            let wire = mesh::fixtures::wire();
            for (fmt, self_contained) in GLTF_FORMS {
                let ctx = format!("{fmt}{}", if self_contained { "" } else { " (package)" });
                let (doc, losses, back) = layer_round_trip(&wire, fmt, self_contained);
                assert!(losses.is_empty(), "{ctx}: {losses:?}");
                let modes: Vec<(&str, Option<u64>)> = doc["meshes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|m| (m["name"].as_str().unwrap(), m.pointer("/primitives/0/mode").and_then(Value::as_u64)))
                    .collect();
                assert_eq!(modes, [("wire_cube", None), ("wire_loop", Some(1)), ("wire_points", Some(0))], "{ctx}");
                for topology in [Topology::Triangles, Topology::Lines, Topology::Points] {
                    assert_eq!(elements(&back.scene, topology), elements(&wire, topology), "{ctx}: {topology:?}");
                }
                assert_eq!(back.scene.drawable_count(), 12 + 4 + 9, "{ctx}");
                let bits = |p: &Primitive| -> Vec<u32> { p.positions.iter().flatten().map(|v| v.to_bits()).collect() };
                for name in ["wire_cube", "wire_loop", "wire_points"] {
                    let (want, got) = (prim_of(&wire, name), prim_of(&back.scene, name));
                    assert_eq!((got.topology, &got.indices, &got.colors), (want.topology, &want.indices, &want.colors), "{ctx}: {name}");
                    assert_eq!(bits(got), bits(want), "{ctx}: {name}'s positions");
                }
            }
        }

        /// A zero normal takes the geometry's, and a line or a point has no
        /// facet to take it from: +Z, not the normal of a triangle made of
        /// whichever three indices come first.
        #[test]
        fn a_zero_normal_on_a_line_or_a_point_is_plus_z() {
            let mut s = Scene::default();
            // Counter-clockwise from -Z: the first three indices, read as a
            // triangle, face -Z.
            let corners = vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
            let normals = Some(vec![[0.0; 3], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
            for (name, topology, indices) in [("lines", Topology::Lines, vec![0, 1, 2, 3]), ("points", Topology::Points, vec![0, 1, 2])] {
                let prim = Primitive { positions: corners.clone(), normals: normals.clone(), indices, topology, ..Default::default() };
                s.meshes.push(SceneMesh { name: Some(name.into()), primitives: vec![prim], ..Default::default() });
                let mesh = s.meshes.len() - 1;
                s.add_root(Node { mesh: Some(mesh), ..Default::default() });
            }
            let written = write_scene(&s);
            assert!(written.is_ok(), "{:?}", written.as_ref().err());
            let back = read_glb(&written.unwrap()).unwrap().scene;
            for name in ["lines", "points"] {
                assert_eq!(prim_of(&back, name).normals.as_ref().map(|n| n[0]), Some([0.0, 0.0, 1.0]), "{name}");
            }
        }

        /// A primitive with one point, or one segment, draws something. Only
        /// a primitive with none of its topology's element is empty, and a
        /// mesh with only those is dropped, with the nodes that draw it.
        #[test]
        fn drop_empty_meshes_judges_each_primitive_by_its_topology() {
            let prim = |topology: Topology, indices: &[u32]| Primitive { positions: QUAD.to_vec(), indices: indices.to_vec(), topology, ..Default::default() };
            let mut s = Scene::default();
            s.meshes.push(SceneMesh {
                primitives: vec![
                    prim(Topology::Triangles, &[0, 1, 2]),
                    prim(Topology::Lines, &[0, 1]),
                    prim(Topology::Points, &[2]),
                    prim(Topology::Triangles, &[]),
                    prim(Topology::Lines, &[]),
                    prim(Topology::Points, &[]),
                ],
                ..Default::default()
            });
            s.meshes.push(SceneMesh { primitives: vec![prim(Topology::Points, &[])], ..Default::default() });
            s.add_root(Node { mesh: Some(0), ..Default::default() });
            s.add_root(Node { mesh: Some(1), ..Default::default() });
            drop_empty_meshes(&mut s);
            let kept: Vec<(Topology, Vec<u32>)> = s.meshes.iter().flat_map(|m| &m.primitives).map(|p| (p.topology, p.indices.clone())).collect();
            assert_eq!(kept, [(Topology::Triangles, vec![0, 1, 2]), (Topology::Lines, vec![0, 1]), (Topology::Points, vec![2])]);
            assert_eq!(s.nodes.iter().map(|n| n.mesh).collect::<Vec<_>>(), [Some(0), None], "the mesh of nothing is gone, and its node draws none");
        }

        // ── the scene-level gates ───────────────────────────────────────────

        /// A glTF of points alone is a scene, not `Empty`. `read_scene` of the
        /// converter decides that by what is drawn, not by its triangles, so
        /// a gate that counts triangles fails this.
        #[test]
        fn a_points_only_glb_reads_as_a_scene() {
            let r = mesh::read_scene(&modes_glb(&[(0, None)]), MeshFormat::Glb, &NoSidecars);
            assert!(r.is_ok(), "{:?}", r.as_ref().err());
            let scene = r.unwrap().scene;
            assert_eq!((scene.triangle_count(), scene.drawable_count()), (0, 4));
            assert_eq!(scene.meshes[0].primitives[0].topology, Topology::Points);
        }

        /// A scene of points alone, and one of lines alone, writes as a GLB
        /// and as a glTF, and reads back as itself. A gate in the writer that
        /// counts triangles refuses them as `Empty`.
        #[test]
        fn a_scene_of_points_or_lines_alone_writes_and_reads_back() {
            let wire = mesh::fixtures::wire();
            for (name, topology) in [("wire_points", Topology::Points), ("wire_loop", Topology::Lines)] {
                let alone = Scene {
                    meshes: vec![wire.meshes.iter().find(|m| m.name.as_deref() == Some(name)).unwrap().clone()],
                    nodes: vec![Node { mesh: Some(0), ..Default::default() }],
                    roots: vec![0],
                    ..Default::default()
                };
                assert_eq!(alone.triangle_count(), 0, "{name}");
                for (fmt, self_contained) in GLTF_FORMS {
                    let ctx = format!("{name} as {fmt}{}", if self_contained { "" } else { " (package)" });
                    let opts = mesh::WriteOptions { self_contained, ..Default::default() };
                    let w = mesh::write_scene(&alone, fmt, opts);
                    assert!(w.is_ok(), "{ctx}: {:?}", w.as_ref().err());
                    let files = w.unwrap().package.files;
                    let back = mesh::read_scene(&files[0].bytes, fmt, &package::MemorySidecars { files: &files });
                    assert!(back.is_ok(), "{ctx}: {:?}", back.as_ref().err());
                    let (want, got) = (&alone.meshes[0].primitives[0], &back.unwrap().scene.meshes[0].primitives[0]);
                    assert_eq!((got.topology, &got.indices, &got.positions, &got.colors), (topology, &want.indices, &want.positions, &want.colors), "{ctx}");
                }
            }
        }

        // ── hostile input ───────────────────────────────────────────────────

        /// One primitive in `mode` whose indices are a sparse accessor of
        /// `count` zeros (one of them 2): a few hundred bytes of file for as
        /// many indices as the claim makes.
        fn sparse_glb(mode: u64, count: usize) -> Vec<u8> {
            let mut bin = f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
            bin.extend_from_slice(&u32s(&[0, 2]));
            let doc = serde_json::json!({
                "asset": {"version": "2.0"},
                "scenes": [{"nodes": [0]}],
                "nodes": [{"mesh": 0}],
                "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1, "mode": mode}]}],
                "accessors": [
                    {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3"},
                    {"componentType": 5125, "count": count, "type": "SCALAR",
                     "sparse": {"count": 1, "indices": {"bufferView": 1, "componentType": 5125}, "values": {"bufferView": 2}}}
                ],
                "bufferViews": [
                    {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                    {"buffer": 0, "byteOffset": 36, "byteLength": 4},
                    {"buffer": 0, "byteOffset": 40, "byteLength": 4}
                ],
                "buffers": [{"byteLength": bin.len()}]
            });
            pack(doc, Some(&bin))
        }

        /// The indices of the one primitive of `sparse_glb(mode, count)`, or
        /// the refusal's detail.
        fn indices_or_refusal(mode: u64, count: usize) -> Result<usize, String> {
            match read_glb(&sparse_glb(mode, count)) {
                Ok(r) => match r.scene.meshes[0].primitives.first() {
                    Some(p) if p.topology == Topology::Lines => Ok(p.indices.len()),
                    other => panic!("mode {mode}, {count} indices: read as {:?}, with warnings {:?}", other.map(|p| p.topology), r.warnings),
                },
                Err(MeshError::Unsupported(d)) => Err(d),
                Err(other) => panic!("mode {mode}, {count} indices: expected Unsupported, got {other:?}"),
            }
        }

        /// Review finding 9's bar for line strips and loops: the ceiling on a
        /// scene's indices counts a strip's segments, which are twice its
        /// vertices less one, and a loop's, twice its vertices, before the
        /// expansion is allocated. A strip whose accessor claims a billion
        /// vertices never reaches the expansion, and one of 7M (a sparse
        /// accessor, so a few hundred bytes of file) is refused for the 14M
        /// indices it stands for, not read. The ceiling itself (12M) still
        /// reads: a strip of 6,000,001 vertices is exactly 12M indices.
        #[test]
        fn a_line_strip_of_a_billion_vertices_is_charged_before_expanding() {
            let ceiling = mesh::scene::MAX_SCENE_INDICES;
            assert_eq!(ceiling, 12_000_000, "the counts below are written for this ceiling");
            let d = indices_or_refusal(3, 1_000_000_000).unwrap_err();
            assert!(d.contains("attribute values"), "a billion vertices are refused by what they would decode to: {d}");
            let d = indices_or_refusal(3, 7_000_000).unwrap_err();
            assert!(d.contains("the merged mesh passes 12000000 indices") && d.contains("primitive 0"), "a strip of 7M: {d}");
            let d = indices_or_refusal(2, 6_000_001).unwrap_err();
            assert!(d.contains("the merged mesh passes 12000000 indices"), "a loop of 6,000,001: {d}");
            assert_eq!(indices_or_refusal(3, 6_000_001), Ok(ceiling), "the strip that is exactly the ceiling");
            assert_eq!(indices_or_refusal(2, 6_000_000), Ok(ceiling), "the loop that is exactly the ceiling");
            let d = indices_or_refusal(3, 6_000_002).unwrap_err();
            assert!(d.contains("the merged mesh passes 12000000 indices"), "one vertex past it: {d}");
        }

        /// The overview's hostile-input bar on the points-and-lines path: a
        /// lean GLB (a four-segment loop and three points, both coloured, as
        /// the writer writes them) read whole, then swept ([`layer_sweep`])
        /// with every bit of its `meshes` array, which holds both primitives'
        /// JSON, among the single flips. The flips reach modes 2 and 3, the
        /// loop and the strip. Every reading is `Ok` or `Err`, never a panic.
        #[test]
        fn glb_points_and_lines_survive_truncation_and_bit_flips() {
            let name = "a lean loop and points as a GLB";
            let mut s = Scene::default();
            let ring = Primitive {
                positions: QUAD.to_vec(),
                colors: Some(vec![[1.0, 0.0, 0.0, 1.0]; 4]),
                indices: vec![0, 1, 1, 2, 2, 3, 3, 0],
                topology: Topology::Lines,
                ..Default::default()
            };
            let dots = Primitive {
                positions: QUAD[..3].to_vec(),
                colors: Some(vec![[0.0, 1.0, 0.0, 1.0]; 3]),
                indices: vec![0, 1, 2],
                topology: Topology::Points,
                ..Default::default()
            };
            s.meshes.push(SceneMesh { name: Some("lean".into()), primitives: vec![ring, dots], ..Default::default() });
            s.add_root(Node { mesh: Some(0), ..Default::default() });
            let file = one_file(&s, MeshFormat::Glb);
            let whole = mesh::read_scene(&file, MeshFormat::Glb, &NoSidecars).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((elements(&whole.scene, Topology::Lines), elements(&whole.scene, Topology::Points)), (4, 3), "{name} itself carries both");
            layer_sweep(name, &file, span(name, &file, r#""meshes":"#, r#""nodes":"#), |bytes| {
                let _ = mesh::read_scene(bytes, MeshFormat::Glb, &NoSidecars);
            });
        }

        /// The same bar on a GLB that has every mode (0 to 3), indexed and
        /// not, so that the loop's and the strip's expansions are swept as
        /// written, not only reached by a flipped bit.
        #[test]
        fn glb_of_every_point_and_line_mode_survives_truncation_and_bit_flips() {
            let name = "points, lines, loops and strips as a GLB";
            let file = modes_glb(&[
                (0, None),
                (0, Some(&[3, 1])),
                (1, None),
                (1, Some(&[0, 2, 1, 3])),
                (2, None),
                (2, Some(&[2, 0, 3])),
                (3, None),
                (3, Some(&[3, 0, 2])),
            ]);
            let whole = read_glb(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!((elements(&whole.scene, Topology::Lines), elements(&whole.scene, Topology::Points)), (16, 6), "{name} itself carries them");
            layer_sweep(name, &file, span(name, &file, r#""meshes":"#, r#""nodes":"#), |bytes| {
                let _ = mesh::read_scene(bytes, MeshFormat::Glb, &NoSidecars);
            });
        }

        /// Ruling I10-R6. KHR_draco_mesh_compression allows TRIANGLES and
        /// TRIANGLE_STRIP only (README :115), and a Draco stream holds
        /// triangles: a Draco primitive in a point or line mode would be read
        /// as the triangles of its stream, not as what its mode says. It is
        /// refused, naming its mode, as a Draco TRIANGLE_FAN is (Box's
        /// glTF-Draco document, which reads as it is).
        #[test]
        fn a_draco_primitive_in_a_point_or_line_mode_is_refused() {
            let whole = read_box(box_draco_doc()).unwrap_or_else(|e| panic!("Box: {e}"));
            assert_eq!(whole.scene.triangle_count(), 12, "Box reads whole, as the triangles it is");
            for (mode, name) in [(0, "POINTS"), (1, "LINES"), (2, "LINE_LOOP"), (3, "LINE_STRIP")] {
                let mut doc = box_draco_doc();
                doc["meshes"][0]["primitives"][0]["mode"] = serde_json::json!(mode);
                let d = match read_box(doc) {
                    Err(MeshError::Malformed(d)) => d,
                    Err(other) => panic!("mode {mode}: expected Malformed, got {other:?}"),
                    Ok(r) => panic!("mode {mode}: read as {:?}, with warnings {:?}", r.scene.meshes[0].primitives.iter().map(|p| (p.topology, p.indices.len())).collect::<Vec<_>>(), r.warnings),
                };
                assert!(
                    d.contains("mesh 0 primitive 0") && d.contains(&format!("a Draco-compressed {name};")) && d.contains("KHR_draco_mesh_compression allows TRIANGLES and TRIANGLE_STRIP"),
                    "mode {mode}: {d}"
                );
            }
        }
    }
}
