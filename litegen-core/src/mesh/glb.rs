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
//! of the selected scene, every triangle primitive with positions, normals,
//! tangents, all UV sets, `COLOR_0`, every `JOINTS_n`/`WEIGHTS_n` pair and morph
//! targets; metallic-roughness materials with `KHR_texture_transform`,
//! `KHR_materials_emissive_strength` and `KHR_materials_unlit`; textures,
//! samplers and images (PNG, JPEG, and WebP/KTX2 carried as-is); skins; and
//! animations of translation, rotation, scale and morph weights.
//!
//! Refused: Draco and meshopt compression (the decoders are separate
//! codecs this module does not carry — reading their placeholder buffers would
//! return a blank model), and point/line primitives, which are skipped because
//! the scene models triangles only.
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
use super::{MeshError, MeshFormat};
use serde_json::Value;
use std::borrow::Cow;
use std::cell::Cell;
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
/// legitimate reaches it.
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
/// model (lights, material variants) or because this reader implements them.
/// Anything NOT on this list is refused: `extensionsRequired` means the file
/// cannot be interpreted correctly without it, and the compression extensions
/// (Draco, meshopt) would otherwise leave us reading a zero-length fallback
/// buffer and reporting success on an empty model.
const IGNORABLE_REQUIRED_EXTENSIONS: &[&str] = &[
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
    "KHR_texture_basisu",
    "KHR_texture_transform",
    "EXT_texture_webp",
];

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
        Some(v) => as_usize(v).map(Some).ok_or_else(|| {
            let shown = v.to_string();
            malformed(format!("{what} is {}, which is not an index", truncate(&shown, 60)))
        }),
    }
}

/// An optional finite number with a default.
fn number_field(v: Option<&Value>, default: f64, what: &str) -> Result<f64, MeshError> {
    match v {
        None => Ok(default),
        Some(v) => v
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| malformed(format!("{what} is {}, which is not a number", truncate(&v.to_string(), 60)))),
    }
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
        out[i] = e
            .as_f64()
            .filter(|f| f.is_finite())
            .ok_or_else(|| malformed(format!("{what}[{i}] is not finite")))?;
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
}

impl Budget {
    fn new() -> Budget {
        Budget { values: Cell::new(0), image_bytes: Cell::new(0) }
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

// ─── document ───────────────────────────────────────────────────────────────

struct Gltf<'a> {
    doc: Value,
    /// Resolved lazily in spirit but eagerly in fact: a buffer that cannot be
    /// resolved stores its error instead of raising it, because a file may
    /// carry an external-URI buffer that only something we never read uses —
    /// and refusing the whole file over a buffer we never read would be wrong.
    buffers: Vec<Result<Cow<'a, [u8]>, MeshError>>,
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
                Some(uri) if uri.len() >= 5 && uri[..5].eq_ignore_ascii_case("data:") => {
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
        Gltf { doc, buffers, quantized, budget: Budget::new() }
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

    /// The bytes of one bufferView, bounded by its own declared extent.
    fn view_bytes(&self, view_index: usize) -> Result<(&[u8], &Value), MeshError> {
        let view = self
            .array("bufferViews")
            .get(view_index)
            .ok_or_else(|| malformed(format!("bufferView index {view_index} out of range")))?;
        if view.pointer("/extensions/EXT_meshopt_compression").is_some() {
            // A meshopt view holds a compressed stream, and the extension is
            // only forced into `extensionsRequired` when its fallback buffer is
            // neither a URI nor the BIN chunk — so a file whose fallback IS the
            // BIN chunk can declare it in `extensionsUsed` alone and walk past
            // the guard above, leaving unspecified placeholder bytes to be read
            // as geometry.
            return Err(unsupported(format!(
                "bufferView {view_index} is EXT_meshopt_compression-encoded"
            )));
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

    fn accessor(&self, index: usize) -> Result<Accessor<'_>, MeshError> {
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
            // Spec §3.6.2.1 does define a view-less accessor as reading zeros,
            // and that is exactly the accessor whose `count` no bytes anywhere
            // bound: a 244-byte file declaring 4 billion of them asked for a
            // 48 GB Vec and took the process down with it. Even honoured, it
            // yields an all-zero attribute — for POSITION a model with no shape,
            // which is not something to hand a customer.
            return Err(unsupported(format!(
                "accessor {index} has no bufferView, which the spec reads as zeros; \
                 zero-filled geometry is not a deliverable model"
            )));
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
            out[i] = v
                .as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| malformed(format!("node {index} matrix entry {i} is not finite")))?;
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
    sidecars: &'g dyn Sidecars,
    total_vertices: usize,
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
            let ws: Vec<f32> = a
                .iter()
                .map(|v| v.as_f64().filter(|f| f.is_finite()).map(|f| f as f32))
                .collect::<Option<_>>()
                .ok_or_else(|| malformed(format!("mesh {gi} weights are not numbers")))?;
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
            // Points and lines genuinely are a different topology, and the
            // scene models nothing but triangles.
            other => {
                self.warnings.push(format!("{where_} has mode {other} (points or lines); it was skipped"));
                return Ok(None);
            }
        }
        if prim.pointer("/extensions/KHR_draco_mesh_compression").is_some() {
            // The uncompressed attributes a Draco primitive points at are a
            // decoy: their bufferViews are allowed to be empty. Reading them
            // would yield a silently blank model.
            return Err(unsupported(format!("{where_} is Draco-compressed")));
        }
        let attrs = prim
            .get("attributes")
            .and_then(|v| v.as_object())
            .ok_or_else(|| malformed(format!("{where_} has no attributes")))?;
        let pos_index = index_field(attrs.get("POSITION"), &format!("{where_} POSITION"))?
            .ok_or_else(|| malformed(format!("{where_} has no POSITION attribute")))?;
        let pos_acc = self.g.accessor(pos_index)?;
        // Checked before the read, not after: a Vec sized past the ceiling is
        // one the allocator may never hand back.
        if self.total_vertices.saturating_add(pos_acc.count) > MAX_MERGED_VERTICES {
            return Err(unsupported(format!(
                "the merged mesh passes {MAX_MERGED_VERTICES} vertices at {where_}; \
                 the document describes more geometry than this converter will hold"
            )));
        }
        let quantized = self.g.quantized;
        // KHR_mesh_quantization allows integer positions, normalised or not;
        // core glTF allows only FLOAT.
        let positions = chunk3(pos_acc.read_f32(&[3], "POSITION", |c, _| {
            c == COMP_FLOAT
                || (quantized && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
        }, &self.g.budget)?);
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
                let v = chunk3(self.g.accessor(i)?.read_f32(&[3], "NORMAL", |c, n| {
                    c == COMP_FLOAT || (quantized && n && matches!(c, COMP_BYTE | COMP_SHORT))
                }, &self.g.budget)?);
                count_ok(v.len(), "normals")?;
                Some(v)
            }
        };
        let tangents = match index_field(attrs.get("TANGENT"), &format!("{where_} TANGENT"))? {
            None => None,
            Some(i) => {
                let v = chunk4(self.g.accessor(i)?.read_f32(&[4], "TANGENT", |c, n| {
                    c == COMP_FLOAT || (quantized && n && matches!(c, COMP_BYTE | COMP_SHORT))
                }, &self.g.budget)?);
                count_ok(v.len(), "tangents")?;
                Some(v)
            }
        };
        let mut uvs = Vec::new();
        for set in 0..MAX_UV_SETS {
            let key = format!("TEXCOORD_{set}");
            let Some(i) = index_field(attrs.get(&key), &format!("{where_} {key}"))? else { break };
            let v = chunk2(self.g.accessor(i)?.read_f32(&[2], &key, |c, n| {
                float_or_unorm(c, n)
                    || (quantized && matches!(c, COMP_BYTE | COMP_UNSIGNED_BYTE | COMP_SHORT | COMP_UNSIGNED_SHORT))
            }, &self.g.budget)?);
            count_ok(v.len(), "uvs")?;
            uvs.push(v);
        }
        let colors = match index_field(attrs.get("COLOR_0"), &format!("{where_} COLOR_0"))? {
            None => None,
            Some(i) => {
                let acc = self.g.accessor(i)?;
                let n = acc.ncomp;
                let raw = acc.read_f32(&[3, 4], "COLOR_0", float_or_unorm, &self.g.budget)?;
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
                    let js = self.g.accessor(j)?.read_u32(4, &jk, &[COMP_UNSIGNED_BYTE, COMP_UNSIGNED_SHORT], &self.g.budget)?;
                    let ws = self.g.accessor(w)?.read_f32(&[4], &wk, float_or_unorm, &self.g.budget)?;
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

        // The vertex order the primitive's mode is written in; what it means as
        // triangles is decided just below.
        let sequence = match index_field(prim.get("indices"), &format!("{where_} indices"))? {
            Some(i) => self.g.accessor(i)?.read_indices(&self.g.budget)?,
            // A primitive with no `indices` walks its vertices in order — for a
            // triangle list that is 0,1,2 then 3,4,5.
            None => {
                if mode == MODE_TRIANGLES && !vcount.is_multiple_of(3) {
                    return Err(malformed(format!(
                        "{where_} is non-indexed with {vcount} vertices, not a multiple of 3"
                    )));
                }
                self.g.budget.charge(vcount, "synthesised indices")?;
                (0..vcount as u32).collect()
            }
        };
        // A strip or a fan is three indices per vertex past the first two, so
        // the ceiling is checked against what the expansion will produce rather
        // than after the Vec has been paid for.
        let expanded = match mode {
            MODE_TRIANGLES => sequence.len(),
            _ => sequence.len().saturating_sub(2).saturating_mul(3),
        };
        if expanded > MAX_MERGED_INDICES {
            return Err(unsupported(format!(
                "the merged mesh passes {MAX_MERGED_INDICES} indices at {where_}"
            )));
        }
        let indices = match mode {
            MODE_TRIANGLE_STRIP => strip_to_triangles(&sequence, &where_)?,
            MODE_TRIANGLE_FAN => fan_to_triangles(&sequence, &where_)?,
            _ => sequence,
        };
        if indices.len() % 3 != 0 {
            return Err(malformed(format!(
                "{where_} has {} indices, not a multiple of 3",
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
        let alpha_mode = match m.get("alphaMode").and_then(|v| v.as_str()) {
            None | Some("OPAQUE") => AlphaMode::Opaque,
            Some("MASK") => AlphaMode::Mask,
            Some("BLEND") => AlphaMode::Blend,
            Some(other) => return Err(malformed(format!("{at} alphaMode {other:?} is not a glTF mode"))),
        };
        let mut out = Material {
            name: m.get("name").and_then(|v| v.as_str()).map(str::to_string),
            base_color: [base[0] as f32, base[1] as f32, base[2] as f32, base[3] as f32],
            metallic: number_field(pbr.and_then(|p| p.get("metallicFactor")), 1.0, &format!("{at} metallicFactor"))? as f32,
            roughness: number_field(pbr.and_then(|p| p.get("roughnessFactor")), 1.0, &format!("{at} roughnessFactor"))? as f32,
            emissive: [
                (emissive[0] * strength) as f32,
                (emissive[1] * strength) as f32,
                (emissive[2] * strength) as f32,
            ],
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
        self.scene.materials.push(out);
        let i = self.scene.materials.len() - 1;
        self.material_map.insert(gi, i);
        Ok(i)
    }

    fn texture_info(&mut self, info: Option<&Value>, at: &str) -> Result<Option<TextureRef>, MeshError> {
        let Some(info) = info else { return Ok(None) };
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
        let Some(texture) = self.texture(ti)? else { return Ok(None) };
        Ok(Some(TextureRef { texture, uv_set, transform }))
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

    fn sampler(&self, si: usize) -> Result<Sampler, MeshError> {
        let s = self
            .g
            .array("samplers")
            .get(si)
            .ok_or_else(|| malformed(format!("sampler index {si} out of range")))?;
        let wrap = |k: &str| -> Result<Wrap, MeshError> {
            Ok(match index_field(s.get(k), &format!("sampler {si} {k}"))? {
                None | Some(10497) => Wrap::Repeat,
                Some(33071) => Wrap::ClampToEdge,
                Some(33648) => Wrap::MirroredRepeat,
                Some(other) => return Err(malformed(format!("sampler {si} {k} is {other}"))),
            })
        };
        let mag = match index_field(s.get("magFilter"), &format!("sampler {si} magFilter"))? {
            None => None,
            Some(9728) => Some(Filter::Nearest),
            Some(9729) => Some(Filter::Linear),
            Some(other) => return Err(malformed(format!("sampler {si} magFilter is {other}"))),
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
                Some(other) => return Err(malformed(format!("sampler {si} minFilter is {other}"))),
            };
        Ok(Sampler {
            wrap_s: wrap("wrapS")?,
            wrap_t: wrap("wrapT")?,
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
            if uri.len() >= 5 && uri[..5].eq_ignore_ascii_case("data:") {
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
                    None | Some("LINEAR") => Interpolation::Linear,
                    Some("STEP") => Interpolation::Step,
                    Some("CUBICSPLINE") => Interpolation::CubicSpline,
                    Some(other) => return Err(malformed(format!("{at} interpolation {other:?} is not a glTF mode"))),
                };
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
                let ch = Channel { node, path, interpolation, times, values };
                let stride = ch.stride(targets);
                let per_key = if interpolation == Interpolation::CubicSpline { stride * 3 } else { stride };
                if ch.values.len() != ch.times.len() * per_key {
                    return Err(malformed(format!(
                        "{at} has {} output values for {} keys of {per_key}",
                        ch.values.len(),
                        ch.times.len()
                    )));
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
                let ws: Option<Vec<f32>> = w.iter().map(|v| v.as_f64().map(|f| f as f32)).collect();
                let targets = n.mesh.map_or(0, |m| self.scene.meshes[m].target_count());
                match ws {
                    Some(ws) if ws.len() == targets && targets > 0 => n.weights = Some(ws),
                    _ => {}
                }
            }
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
}

/// What a read produced besides the scene: things in the file that were
/// recognised and deliberately not carried, in words.
pub struct ReadOutcome {
    pub scene: Scene,
    pub warnings: Vec<String>,
}

fn read_document(doc: Value, bin: Option<&[u8]>, sidecars: &dyn Sidecars) -> Result<ReadOutcome, MeshError> {
    check_required_extensions(&doc)?;
    let g = Gltf::new(doc, bin, sidecars);
    let mut r = Reader {
        g: &g,
        scene: Scene::default(),
        warnings: Vec::new(),
        mesh_map: HashMap::new(),
        material_map: HashMap::new(),
        texture_map: HashMap::new(),
        image_map: HashMap::new(),
        skin_map: HashMap::new(),
        node_map: HashMap::new(),
        sidecars,
        total_vertices: 0,
    };
    r.nodes()?;
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
    let Reader { scene, warnings, .. } = r;
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

/// Move every skinned mesh to an untransformed root node.
///
/// glTF says a skinned mesh node's own transform is ignored; three.js (the
/// engine behind `<model-viewer>`) multiplies it in anyway when binding, so a
/// file whose skinned node carries a transform renders differently in the
/// product's own viewer than the spec says. Moving the mesh to a fresh root
/// with identity transform changes nothing under the spec and makes every
/// reader agree. The original node keeps its name, transform and children.
pub fn hoist_skinned_meshes(scene: &Scene) -> Scene {
    let mut out = scene.clone();
    let world = scene.world_matrices();
    let parents = scene.parents();
    for i in 0..scene.nodes.len() {
        let n = &scene.nodes[i];
        let (Some(mesh), Some(skin)) = (n.mesh, n.skin) else { continue };
        let at_identity_root = parents[i].is_none()
            && world[i].iter().zip(math::IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-12);
        if at_identity_root {
            continue;
        }
        let weights = out.nodes[i].weights.take();
        out.nodes[i].mesh = None;
        out.nodes[i].skin = None;
        let name = n.name.clone().map(|s| format!("{s}_skinned"));
        let new = out.add_root(Node { name, mesh: Some(mesh), skin: Some(skin), weights, ..Default::default() });
        // Morph-weight animation targets the node that holds the mesh.
        for a in out.animations.iter_mut() {
            for c in a.channels.iter_mut() {
                if c.node == i && c.path == AnimPath::Weights {
                    c.node = new;
                }
            }
        }
    }
    out
}

fn build_document(scene: &Scene, placement: ImagePlacement) -> Result<Written, MeshError> {
    scene.validate()?;
    let scene = &hoist_skinned_meshes(scene);
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
                let derived = super::pose::derived_normals(&p.positions, &p.indices);
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
                while used_names.contains(&name) || name == "model.bin" {
                    name = format!("{stem}_{n}.{}", im.format.extension());
                    n += 1;
                }
                used_names.push(name.clone());
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
        if !exts.is_empty() {
            mj.insert("extensions".into(), Value::Object(exts));
        }
        materials_json.push(Value::Object(mj));
    }

    // Skins.
    let mut skins_json = Vec::new();
    for s in &scene.skins {
        let flat: Vec<f32> = s.inverse_bind_matrices.iter().flatten().map(|v| *v as f32).collect();
        let acc = b.floats(&flat, 16, None, false);
        let mut sj = serde_json::json!({ "joints": s.joints, "inverseBindMatrices": acc });
        if let Some(k) = s.skeleton {
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
        nodes_json.push(Value::Object(nj));
    }

    // Animations: one sampler per channel, input accessors shared when the key
    // times are identical (the common "every channel keyed on every frame").
    let mut animations_json = Vec::new();
    let mut time_accessors: Vec<(Vec<f32>, usize)> = Vec::new();
    for a in &scene.animations {
        let mut samplers = Vec::new();
        let mut channels = Vec::new();
        for ch in &a.channels {
            let input = match time_accessors.iter().find(|(t, _)| *t == ch.times) {
                Some((_, acc)) => *acc,
                None => {
                    let acc = b.floats(&ch.times, 1, None, true);
                    time_accessors.push((ch.times.clone(), acc));
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
    put("accessors", std::mem::take(&mut b.accessors));
    put("bufferViews", std::mem::take(&mut b.views));
    if !b.bin.is_empty() {
        obj.insert("buffers".into(), serde_json::json!([{ "byteLength": b.bin.len() }]));
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
    if scene.triangle_count() == 0 {
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
    if scene.triangle_count() == 0 {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn skips_primitives_that_are_not_triangles() {
        let (mut doc, bin) = triangle_doc();
        let prim = doc["meshes"][0]["primitives"][0].clone();
        let mut line = prim.clone();
        line["mode"] = Value::from(1);
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

    #[test]
    fn rejects_a_primitive_without_positions() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]["attributes"] = serde_json::json!({});
        let d = unwrap_malformed(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("no POSITION"), "{d}");
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

    #[test]
    fn rejects_a_required_compression_extension() {
        let (mut doc, bin) = triangle_doc();
        doc["extensionsRequired"] = serde_json::json!(["KHR_draco_mesh_compression"]);
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("KHR_draco_mesh_compression"), "{d}");
    }

    #[test]
    fn ignores_a_required_material_only_extension() {
        let (mut doc, bin) = triangle_doc();
        doc["extensionsRequired"] = serde_json::json!(["KHR_materials_unlit"]);
        assert_eq!(read(&pack(doc, Some(&bin))).unwrap().triangle_count(), 1);
    }

    #[test]
    fn rejects_a_draco_primitive_even_when_it_is_not_declared_required() {
        let (mut doc, bin) = triangle_doc();
        doc["meshes"][0]["primitives"][0]["extensions"] =
            serde_json::json!({"KHR_draco_mesh_compression": {"bufferView": 0, "attributes": {}}});
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("Draco"), "{d}");
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

    #[test]
    fn refuses_an_accessor_without_a_buffer_view() {
        // Spec §3.6.2.1 reads it as zeros. Honouring that gives an all-zero
        // POSITION — a model with no shape — and leaves `count` backed by no
        // bytes at all, which is the hole the next test walks through.
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0].as_object_mut().unwrap().remove("bufferView");
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("no bufferView"), "{d}");
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

    #[test]
    fn rejects_a_meshopt_buffer_view_declared_only_as_used() {
        // EXT_meshopt_compression is forced into extensionsRequired only when
        // its fallback buffer is neither a URI nor the BIN chunk, so a file
        // whose fallback IS the BIN chunk can declare it in extensionsUsed alone
        // and have its unspecified placeholder bytes read as geometry.
        let (mut doc, bin) = triangle_doc();
        doc["extensionsUsed"] = serde_json::json!(["EXT_meshopt_compression"]);
        doc["bufferViews"][0]["extensions"] = serde_json::json!({
            "EXT_meshopt_compression": {"buffer": 0, "byteLength": 36, "mode": "ATTRIBUTES"}
        });
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("EXT_meshopt_compression"), "{d}");
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

    #[test]
    fn rejects_a_strip_or_fan_too_short_to_be_a_triangle() {
        let two = [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0]];
        for mode in [MODE_TRIANGLE_STRIP, MODE_TRIANGLE_FAN] {
            let d = unwrap_malformed(read(&mode_glb(mode, &two)).unwrap_err());
            assert!(d.contains("at least 3"), "mode {mode}: {d}");
        }
    }

    #[test]
    fn still_drops_points_and_lines() {
        let (mut doc, bin) = triangle_doc();
        let prim = doc["meshes"][0]["primitives"][0].clone();
        for mode in [0, 1, 2, 3] {
            let mut other = prim.clone();
            other["mode"] = Value::from(mode);
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
}
