//! glTF 2.0 binary (`.glb`) — read and write.
//!
//! # Why this reader is the paranoid one
//!
//! GLB is the container every 3D vendor in the catalog actually emits, so these
//! bytes are the ones that arrive from outside. Everything here assumes the
//! input is a real exporter's output rather than our own writer's: a scene graph
//! with a baked root rotation, geometry split into one primitive per material,
//! interleaved vertex buffers, and chunk types we have never heard of. Reading
//! any of that wrongly does not fail — it hands the customer a model that is
//! rotated, half-missing or inside out — which is why the traversal accumulates
//! transforms, merges every primitive, and refuses to guess.
//!
//! The other half of the paranoia is the hostile case. A `count`, a
//! `byteLength` or a chunk length in this file is a 32-bit number chosen by
//! someone else, so every index and every length is checked before it is used;
//! a truncated or malicious GLB must come back as `Err`, never as a panic.
//!
//! @see <https://registry.khronos.org/glTF/specs/2.0/glTF-2.0.html#binary-gltf-layout>

use super::ir::Mesh;
use super::{MeshError, MeshFormat};
use serde_json::Value;
use std::borrow::Cow;

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

/// Ceiling on the merged model, in vertices and in indices.
///
/// Nothing else bounds these two arrays. Geometry is re-read once per node
/// instance and once per primitive, so one 20k-vertex accessor reached from 500
/// nodes is 10M merged vertices out of a 248 KB file — `super::MAX_INPUT_BYTES`
/// caps the bytes that arrive, not the mesh they describe.
///
/// 4M is what a 128 MB GLB can honestly carry at the ceiling: interleaved
/// position, normal and uv is 32 bytes a vertex, so nothing legitimate that
/// passes `MAX_INPUT_BYTES` has more. It pins the merged arrays at roughly
/// 180 MB, which fails loudly here instead of quietly in the allocator — whose
/// own failure mode is to abort the process rather than return.
const MAX_MERGED_VERTICES: usize = 4_000_000;
const MAX_MERGED_INDICES: usize = 12_000_000;

/// Extensions that may appear in `extensionsRequired` without invalidating what
/// we extract. All of them describe materials or lights — the things this module
/// drops anyway (see the lossiness table in `super`) — so ignoring them changes
/// no vertex. Anything NOT on this list is refused: `extensionsRequired` means
/// the file cannot be interpreted correctly without it, and the compression
/// extensions (Draco, meshopt, quantization) would otherwise leave us reading a
/// zero-length fallback buffer and reporting success on an empty model.
const IGNORABLE_REQUIRED_EXTENSIONS: &[&str] = &[
    "KHR_lights_punctual",
    "KHR_materials_clearcoat",
    "KHR_materials_emissive_strength",
    "KHR_materials_ior",
    "KHR_materials_iridescence",
    "KHR_materials_sheen",
    "KHR_materials_specular",
    "KHR_materials_transmission",
    "KHR_materials_unlit",
    "KHR_materials_variants",
    "KHR_materials_volume",
    "KHR_texture_basisu",
    "KHR_texture_transform",
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

/// Callers must have bounds-checked `off + 4`; every call site reads inside an
/// element slice whose size was derived from the component count.
fn f32_le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
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

// ─── 4x4 column-major maths ─────────────────────────────────────────────────
//
// glTF stores `matrix` column-major, and node transforms compose down the
// hierarchy, so the accumulation happens in f64: a chain of five nodes each
// with a rotation and a scale loses visible precision in f32 on large
// coordinates, and the result is a mesh whose seams no longer weld.

type Mat4 = [f64; 16];

const IDENTITY: Mat4 = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// `a * b`, i.e. apply `b` first. Element (row r, col c) lives at `[c * 4 + r]`.
fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0f64; 16];
    for c in 0..4 {
        for r in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + r] * b[c * 4 + k];
            }
            out[c * 4 + r] = sum;
        }
    }
    out
}

fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (p[0] as f64, p[1] as f64, p[2] as f64);
    let mut out = [0.0f32; 3];
    for r in 0..3 {
        out[r] = (m[r] * x + m[4 + r] * y + m[8 + r] * z + m[12 + r]) as f32;
    }
    out
}

/// Upper-left 3x3, column-major, as `[c * 3 + r]`.
fn upper3(m: &Mat4) -> [f64; 9] {
    let mut u = [0.0f64; 9];
    for c in 0..3 {
        for r in 0..3 {
            u[c * 3 + r] = m[c * 4 + r];
        }
    }
    u
}

fn det3(u: &[f64; 9]) -> f64 {
    u[0] * (u[4] * u[8] - u[7] * u[5]) - u[3] * (u[1] * u[8] - u[7] * u[2])
        + u[6] * (u[1] * u[5] - u[4] * u[2])
}

/// The matrix that carries normals: inverse-transpose of the upper 3x3.
///
/// The plain 3x3 is only correct under uniform scale. Vendors bake non-uniform
/// scale into a root node often enough (a mirrored `[-1, 1, 1]` is the usual
/// one) that using it would hand back a model whose normals point sideways on
/// stretched geometry and inward on mirrored geometry. When the transform is
/// singular there is no right answer, so fall back to the plain 3x3 rather than
/// propagate NaN through every vertex.
fn normal_matrix(m: &Mat4) -> [f64; 9] {
    let u = upper3(m);
    let det = det3(&u);
    if det.abs() < 1e-12 || !det.is_finite() {
        return u;
    }
    // adjugate / det, transposed — written out because a 3x3 inverse is shorter
    // than the machinery to do it generically.
    let inv = [
        (u[4] * u[8] - u[7] * u[5]) / det,
        (u[7] * u[2] - u[1] * u[8]) / det,
        (u[1] * u[5] - u[4] * u[2]) / det,
        (u[6] * u[5] - u[3] * u[8]) / det,
        (u[0] * u[8] - u[6] * u[2]) / det,
        (u[3] * u[2] - u[0] * u[5]) / det,
        (u[3] * u[7] - u[6] * u[4]) / det,
        (u[6] * u[1] - u[0] * u[7]) / det,
        (u[0] * u[4] - u[3] * u[1]) / det,
    ];
    // transpose of the inverse
    let mut out = [0.0f64; 9];
    for c in 0..3 {
        for r in 0..3 {
            out[c * 3 + r] = inv[r * 3 + c];
        }
    }
    out
}

fn transform_normal(u: &[f64; 9], n: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (n[0] as f64, n[1] as f64, n[2] as f64);
    let mut v = [0.0f64; 3];
    for r in 0..3 {
        v[r] = u[r] * x + u[3 + r] * y + u[6 + r] * z;
    }
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 0.0 && len.is_finite() {
        [(v[0] / len) as f32, (v[1] / len) as f32, (v[2] / len) as f32]
    } else {
        // A normal that collapsed to zero under the transform carries no
        // direction; emitting it unchanged is the least-wrong option and keeps
        // the array the length `Mesh::validate` requires.
        n
    }
}

/// TRS composed the way the spec defines it: `M = T * R * S`.
fn trs_matrix(t: [f64; 3], q: [f64; 4], s: [f64; 3]) -> Mat4 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let r = [
        1.0 - 2.0 * (y * y + z * z),
        2.0 * (x * y + z * w),
        2.0 * (x * z - y * w),
        2.0 * (x * y - z * w),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + x * w),
        2.0 * (x * z + y * w),
        2.0 * (y * z - x * w),
        1.0 - 2.0 * (x * x + y * y),
    ]; // column-major [c * 3 + r]
    [
        r[0] * s[0], r[1] * s[0], r[2] * s[0], 0.0, //
        r[3] * s[1], r[4] * s[1], r[5] * s[1], 0.0, //
        r[6] * s[2], r[7] * s[2], r[8] * s[2], 0.0, //
        t[0], t[1], t[2], 1.0,
    ]
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
    let doc: Value = serde_json::from_slice(&chunk[..end])
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

// ─── accessors ──────────────────────────────────────────────────────────────

/// One accessor resolved down to "where the bytes are and how to step them".
struct Accessor<'a> {
    index: usize,
    /// The bytes of its `bufferView` and nothing either side of them.
    ///
    /// Holding the whole buffer here and bounding reads against that was how a
    /// view declaring one vertex could serve three: the extra 24 bytes came out
    /// of whatever the next view held — usually the index array — and the caller
    /// got plausible wrong geometry instead of an error.
    data: &'a [u8],
    start: usize,
    stride: usize,
    elem_size: usize,
    comp: u32,
    ncomp: usize,
    count: usize,
    type_name: String,
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

    /// `count` is attacker-controlled, so never reserve on it directly — a file
    /// declaring 4 billion elements would allocate before the first bounds check
    /// could reject it. `Gltf::accessor` now proves `count` against the view's
    /// bytes before this is ever reached; the cap stays because being wrong about
    /// that costs the process rather than the request.
    fn capacity(&self) -> usize {
        self.count.min(4096)
    }

    fn require(&self, want_comp: u32, want_ncomp: usize, what: &str) -> Result<(), MeshError> {
        if self.ncomp != want_ncomp {
            return Err(malformed(format!(
                "{what} accessor {} is {}, expected {}",
                self.index,
                self.type_name,
                match want_ncomp {
                    1 => "SCALAR",
                    2 => "VEC2",
                    _ => "VEC3",
                }
            )));
        }
        if self.comp != want_comp {
            // Quantized attributes (KHR_mesh_quantization) land here. They are a
            // real encoding, not corruption, and reading them as floats would
            // silently produce garbage coordinates — so they are Unsupported.
            return Err(unsupported(format!(
                "{what} accessor {} uses componentType {}; only FLOAT (5126) is supported",
                self.index, self.comp
            )));
        }
        Ok(())
    }

    fn read_vec3(&self, what: &str) -> Result<Vec<[f32; 3]>, MeshError> {
        self.require(COMP_FLOAT, 3, what)?;
        let mut out = Vec::with_capacity(self.capacity());
        for i in 0..self.count {
            let e = self.element(i)?;
            out.push([f32_le(e, 0), f32_le(e, 4), f32_le(e, 8)]);
        }
        Ok(out)
    }

    fn read_vec2(&self, what: &str) -> Result<Vec<[f32; 2]>, MeshError> {
        self.require(COMP_FLOAT, 2, what)?;
        let mut out = Vec::with_capacity(self.capacity());
        for i in 0..self.count {
            let e = self.element(i)?;
            out.push([f32_le(e, 0), f32_le(e, 4)]);
        }
        Ok(out)
    }

    fn read_indices(&self) -> Result<Vec<u32>, MeshError> {
        if self.ncomp != 1 {
            return Err(malformed(format!(
                "index accessor {} is {}, expected SCALAR",
                self.index, self.type_name
            )));
        }
        let mut out = Vec::with_capacity(self.capacity());
        for i in 0..self.count {
            let e = self.element(i)?;
            let v = match self.comp {
                COMP_UNSIGNED_BYTE => e[0] as u32,
                COMP_UNSIGNED_SHORT => u16::from_le_bytes([e[0], e[1]]) as u32,
                COMP_UNSIGNED_INT => u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
                // The spec permits exactly those three; a signed or float index
                // accessor is a broken file, not a feature we declined.
                other => {
                    return Err(malformed(format!(
                        "index accessor {} uses componentType {other}; \
                         glTF allows only UNSIGNED_BYTE, UNSIGNED_SHORT and UNSIGNED_INT",
                        self.index
                    )))
                }
            };
            out.push(v);
        }
        Ok(out)
    }
}

// ─── document ───────────────────────────────────────────────────────────────

struct Gltf<'a> {
    doc: Value,
    /// Resolved lazily in spirit but eagerly in fact: a buffer that cannot be
    /// resolved stores its error instead of raising it, because a GLB may carry
    /// an external-URI buffer that only its textures reference — and refusing
    /// the whole file over a buffer we never read would be wrong.
    buffers: Vec<Result<Cow<'a, [u8]>, MeshError>>,
}

impl<'a> Gltf<'a> {
    fn new(doc: Value, bin: Option<&'a [u8]>) -> Self {
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
                Some(uri) if uri.starts_with("data:") || uri.starts_with("DATA:") => {
                    decode_data_uri(uri, i)
                }
                Some(uri) => Err(unsupported(format!(
                    "buffer {i} points at the external file {:?}; \
                     conversion runs without a filesystem, so only embedded buffers work",
                    truncate(uri, 120)
                ))),
            })
            .collect();
        Gltf { doc, buffers }
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

    fn accessor(&self, index: usize) -> Result<Accessor<'_>, MeshError> {
        let a = self
            .array("accessors")
            .get(index)
            .ok_or_else(|| malformed(format!("accessor index {index} out of range")))?;
        if a.get("sparse").is_some() {
            // Sparse accessors overlay a base accessor with a scattered patch.
            // Implementing them means a second index/value pair per accessor and
            // a merge step; nothing in the vendor set emits them, so they are an
            // honest refusal rather than a half-applied overlay.
            return Err(unsupported(format!("accessor {index} is sparse")));
        }
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
            "MAT2" | "MAT3" | "MAT4" => {
                return Err(unsupported(format!(
                    "accessor {index} is {type_name}; matrix accessors only appear on \
                     skins and morph targets, which this converter does not model"
                )))
            }
            other => return Err(malformed(format!("accessor {index} has type {other:?}"))),
        };
        let count = a
            .get("count")
            .and_then(as_usize)
            .ok_or_else(|| malformed(format!("accessor {index} has no count")))?;
        let elem_size = ncomp * comp_size;
        let acc_offset =
            index_field(a.get("byteOffset"), &format!("accessor {index} byteOffset"))?.unwrap_or(0);

        let Some(view_index) =
            index_field(a.get("bufferView"), &format!("accessor {index} bufferView"))?
        else {
            // Spec §3.6.2.1 does define a view-less accessor as reading zeros,
            // and that is exactly the accessor whose `count` no bytes anywhere
            // bound: a 244-byte file declaring 4 billion of them asked for a
            // 48 GB Vec and took the process down with it. Even honoured, it
            // yields an all-zero POSITION — a model with no shape, which is not
            // something to hand a customer.
            return Err(unsupported(format!(
                "accessor {index} has no bufferView, which the spec reads as zeros; \
                 zero-filled geometry is not a deliverable model"
            )));
        };
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
            start: acc_offset,
            stride,
            elem_size,
            comp,
            ncomp,
            count,
            type_name,
        })
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

fn decode_data_uri(uri: &str, buffer_index: usize) -> Result<Cow<'static, [u8]>, MeshError> {
    let comma = uri
        .find(',')
        .ok_or_else(|| malformed(format!("buffer {buffer_index} has a data: uri with no comma")))?;
    let meta = &uri[..comma];
    if !meta.to_ascii_lowercase().contains(";base64") {
        // Percent-encoded data URIs are legal and essentially never emitted for
        // binary geometry; decoding them is a second decoder's worth of attack
        // surface for no vendor.
        return Err(unsupported(format!(
            "buffer {buffer_index} uses a non-base64 data: uri"
        )));
    }
    let payload = &uri[comma + 1..];
    decode_base64(payload)
        .map(Cow::Owned)
        .ok_or_else(|| malformed(format!("buffer {buffer_index} has an invalid base64 payload")))
}

// ─── scene traversal ────────────────────────────────────────────────────────

/// One mesh reached through the node graph, with the world matrix that puts it
/// where the file says it belongs.
struct Instance {
    mesh: usize,
    world: Mat4,
}

fn node_local_matrix(node: &Value, index: usize) -> Result<Mat4, MeshError> {
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
            let f = v
                .as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| malformed(format!("node {index} matrix entry {i} is not finite")))?;
            out[i] = f;
        }
        return Ok(out);
    }
    let vec3 = |key: &str, default: f64| -> Result<[f64; 3], MeshError> {
        match node.get(key) {
            None => Ok([default; 3]),
            Some(v) => {
                let a = v
                    .as_array()
                    .filter(|a| a.len() == 3)
                    .ok_or_else(|| malformed(format!("node {index} {key} is not a 3-array")))?;
                let mut out = [0.0f64; 3];
                for (i, e) in a.iter().enumerate() {
                    out[i] = e.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                        malformed(format!("node {index} {key}[{i}] is not finite"))
                    })?;
                }
                Ok(out)
            }
        }
    };
    let t = vec3("translation", 0.0)?;
    let s = vec3("scale", 1.0)?;
    let q = match node.get("rotation") {
        None => [0.0, 0.0, 0.0, 1.0],
        Some(v) => {
            let a = v
                .as_array()
                .filter(|a| a.len() == 4)
                .ok_or_else(|| malformed(format!("node {index} rotation is not a 4-array")))?;
            let mut q = [0.0f64; 4];
            for (i, e) in a.iter().enumerate() {
                q[i] = e.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                    malformed(format!("node {index} rotation[{i}] is not finite"))
                })?;
            }
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
    Ok(trs_matrix(t, q, s))
}

impl Gltf<'_> {
    fn instances(&self) -> Result<Vec<Instance>, MeshError> {
        let scenes = self.array("scenes");
        let nodes = self.array("nodes");
        if scenes.is_empty() {
            // A glTF with meshes but no scene is legal (the spec calls it a
            // library) and a few pipelines emit one. Dropping every mesh would
            // report "empty" for a file that plainly has geometry, so take them
            // all at identity — there is no hierarchy to honour.
            return Ok((0..self.array("meshes").len())
                .map(|mesh| Instance { mesh, world: IDENTITY })
                .collect());
        }
        let scene_index = index_field(self.doc.get("scene"), "scene")?.unwrap_or(0);
        let scene = scenes
            .get(scene_index)
            .ok_or_else(|| malformed(format!("scene index {scene_index} out of range")))?;
        let roots = scene.get("nodes").and_then(|v| v.as_array()).map_or(&[][..], |v| v.as_slice());

        let mut out = Vec::new();
        let mut visited = vec![false; nodes.len()];
        // Explicit stack, not recursion: node depth is attacker-controlled and a
        // 100k-deep chain would blow the process stack rather than return Err.
        let mut stack: Vec<(usize, Mat4)> = Vec::new();
        for r in roots.iter().rev() {
            let i = r
                .as_u64()
                .and_then(|v| usize::try_from(v).ok())
                .ok_or_else(|| malformed("scene node reference is not an index"))?;
            stack.push((i, IDENTITY));
        }
        while let Some((index, parent)) = stack.pop() {
            let node = nodes
                .get(index)
                .ok_or_else(|| malformed(format!("node index {index} out of range")))?;
            // glTF node graphs are strict forests. A node reached twice is
            // either a cycle (which would loop forever) or a shared subtree
            // (which would silently duplicate geometry) — both are the file
            // lying about its structure, so say so instead of coping.
            if std::mem::replace(&mut visited[index], true) {
                return Err(malformed(format!(
                    "node {index} is reachable more than once; the node graph is not a tree"
                )));
            }
            let world = mat_mul(&parent, &node_local_matrix(node, index)?);
            // A `mesh` that does not read as an index is this node's geometry
            // going missing without a word, so it is the file's error and not a
            // node that happens to carry no mesh.
            if let Some(mesh) = index_field(node.get("mesh"), &format!("node {index} mesh"))? {
                out.push(Instance { mesh, world });
            }
            if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
                for c in children.iter().rev() {
                    let ci = c
                        .as_u64()
                        .and_then(|v| usize::try_from(v).ok())
                        .ok_or_else(|| malformed(format!("node {index} child is not an index")))?;
                    stack.push((ci, world));
                }
            }
        }
        Ok(out)
    }
}

// ─── primitive merge ────────────────────────────────────────────────────────

/// Where one primitive's vertices landed in the merged arrays, so the normal
/// fix-up below can find them again.
struct Span {
    vertex_offset: usize,
    vertex_count: usize,
    index_offset: usize,
    index_count: usize,
    had_normals: bool,
}

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

/// Area-weighted vertex normals, for a primitive that shipped without any while
/// its siblings had them (see the merge comment below).
fn derived_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f64; 3]; positions.len()];
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (Some(pa), Some(pb), Some(pc)) =
            (positions.get(a), positions.get(b), positions.get(c))
        else {
            continue;
        };
        let u = [
            (pb[0] - pa[0]) as f64,
            (pb[1] - pa[1]) as f64,
            (pb[2] - pa[2]) as f64,
        ];
        let w = [
            (pc[0] - pa[0]) as f64,
            (pc[1] - pa[1]) as f64,
            (pc[2] - pa[2]) as f64,
        ];
        // Unnormalised cross product = twice the triangle area, which is exactly
        // the weight a smooth normal wants.
        let n = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        for v in [a, b, c] {
            for k in 0..3 {
                acc[v][k] += n[k];
            }
        }
    }
    acc.into_iter()
        .map(|n| {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 0.0 && len.is_finite() {
                [(n[0] / len) as f32, (n[1] / len) as f32, (n[2] / len) as f32]
            } else {
                // An unreferenced or degenerate vertex has no geometric normal;
                // +Z keeps the array unit-length, which glTF requires.
                [0.0, 0.0, 1.0]
            }
        })
        .collect()
}

impl Gltf<'_> {
    fn base_color_of(&self, prim: &Value) -> Result<Option<[f32; 4]>, MeshError> {
        let Some(mi) = index_field(prim.get("material"), "primitive material")? else {
            return Ok(None);
        };
        let material = self
            .array("materials")
            .get(mi)
            .ok_or_else(|| malformed(format!("material index {mi} out of range")))?;
        let Some(f) = material.pointer("/pbrMetallicRoughness/baseColorFactor") else {
            return Ok(None);
        };
        let a = f
            .as_array()
            .filter(|a| a.len() == 4)
            .ok_or_else(|| malformed(format!("material {mi} baseColorFactor is not a 4-array")))?;
        let mut out = [0.0f32; 4];
        for (i, v) in a.iter().enumerate() {
            out[i] = v
                .as_f64()
                .filter(|f| f.is_finite())
                .ok_or_else(|| malformed(format!("material {mi} baseColorFactor[{i}] is not finite")))?
                as f32;
        }
        Ok(Some(out))
    }

    fn to_mesh(&self) -> Result<Mesh, MeshError> {
        let meshes = self.array("meshes");
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        let mut any_normals = false;
        let mut any_uvs = false;
        let mut base_color: Option<[f32; 4]> = None;
        let mut name: Option<String> = None;

        for inst in self.instances()? {
            let mesh = meshes
                .get(inst.mesh)
                .ok_or_else(|| malformed(format!("mesh index {} out of range", inst.mesh)))?;
            let nmat = normal_matrix(&inst.world);
            // A negative-determinant transform (a mirror) reverses which side of
            // each triangle faces out. glTF says the winding MUST flip to
            // compensate; skipping this leaves a mirrored model rendering as its
            // own inside-out shell under default back-face culling.
            let mirrored = det3(&upper3(&inst.world)) < 0.0;

            for (pi, prim) in mesh
                .get("primitives")
                .and_then(|v| v.as_array())
                .map_or(&[][..], |v| v.as_slice())
                .iter()
                .enumerate()
            {
                let where_ = format!("mesh {} primitive {pi}", inst.mesh);
                let mode = match prim.get("mode") {
                    None => MODE_TRIANGLES,
                    Some(m) => m
                        .as_u64()
                        .ok_or_else(|| malformed(format!("{where_} has a non-numeric mode")))?,
                };
                match mode {
                    // A strip and a fan are a triangle list written down more
                    // compactly, and the spec says exactly which triangles they
                    // stand for — there is nothing here to guess, so skipping
                    // them was not caution, it was handing back half a model.
                    MODE_TRIANGLES | MODE_TRIANGLE_STRIP | MODE_TRIANGLE_FAN => {}
                    // Points and lines genuinely are a different topology, and
                    // the IR models nothing but triangles.
                    _ => continue,
                }
                if prim
                    .pointer("/extensions/KHR_draco_mesh_compression")
                    .is_some()
                {
                    // The uncompressed attributes a Draco primitive points at
                    // are a decoy: their bufferViews are allowed to be empty.
                    // Reading them would yield a silently blank model.
                    return Err(unsupported(format!("{where_} is Draco-compressed")));
                }

                let attrs = prim
                    .get("attributes")
                    .and_then(|v| v.as_object())
                    .ok_or_else(|| malformed(format!("{where_} has no attributes")))?;
                let pos_acc = index_field(attrs.get("POSITION"), &format!("{where_} POSITION"))?
                    .ok_or_else(|| malformed(format!("{where_} has no POSITION attribute")))?;
                let pos_acc = self.accessor(pos_acc)?;
                // Checked before the read, not after: the merge is what a single
                // small accessor multiplies through, and a Vec sized past the
                // ceiling is one the allocator may never hand back.
                if positions.len().saturating_add(pos_acc.count) > MAX_MERGED_VERTICES {
                    return Err(unsupported(format!(
                        "the merged mesh passes {MAX_MERGED_VERTICES} vertices at {where_}; \
                         the node graph instances more geometry than this converter will hold"
                    )));
                }
                let local_positions = pos_acc.read_vec3("POSITION")?;
                let vcount = local_positions.len();

                let local_normals = match index_field(attrs.get("NORMAL"), &format!("{where_} NORMAL"))? {
                    None => None,
                    Some(i) => {
                        let a = self.accessor(i)?;
                        let v = a.read_vec3("NORMAL")?;
                        if v.len() != vcount {
                            return Err(malformed(format!(
                                "{where_} has {} normals for {vcount} positions",
                                v.len()
                            )));
                        }
                        Some(v)
                    }
                };
                let local_uvs = match index_field(attrs.get("TEXCOORD_0"), &format!("{where_} TEXCOORD_0"))? {
                    None => None,
                    Some(i) => {
                        let a = self.accessor(i)?;
                        // Normalised byte/short UVs are legal glTF. Decoding
                        // them is easy; deciding what a normalised SIGNED byte
                        // means at the -128 endpoint is not, and no vendor in
                        // the set emits them, so they are refused out loud.
                        let v = a.read_vec2("TEXCOORD_0")?;
                        if v.len() != vcount {
                            return Err(malformed(format!(
                                "{where_} has {} uvs for {vcount} positions",
                                v.len()
                            )));
                        }
                        Some(v)
                    }
                };

                // The vertex order the primitive's mode is written in; what it
                // means as triangles is decided just below.
                let sequence = match index_field(prim.get("indices"), &format!("{where_} indices"))? {
                    Some(i) => self.accessor(i)?.read_indices()?,
                    // A primitive with no `indices` walks its vertices in order
                    // — for a triangle list that is 0,1,2 then 3,4,5.
                    None => {
                        if mode == MODE_TRIANGLES && !vcount.is_multiple_of(3) {
                            return Err(malformed(format!(
                                "{where_} is non-indexed with {vcount} vertices, not a multiple of 3"
                            )));
                        }
                        (0..vcount as u32).collect()
                    }
                };
                // A strip or a fan is three indices per vertex past the first
                // two, so the ceiling is checked against what the expansion will
                // produce rather than after the Vec has been paid for. Indices
                // are the array that runs away first on a strip-heavy file: the
                // vertex ceiling above bounds them only loosely.
                let expanded = match mode {
                    MODE_TRIANGLES => sequence.len(),
                    _ => sequence.len().saturating_sub(2).saturating_mul(3),
                };
                if indices.len().saturating_add(expanded) > MAX_MERGED_INDICES {
                    return Err(unsupported(format!(
                        "the merged mesh passes {MAX_MERGED_INDICES} indices at {where_}"
                    )));
                }
                let mut local_indices = match mode {
                    MODE_TRIANGLE_STRIP => strip_to_triangles(&sequence, &where_)?,
                    MODE_TRIANGLE_FAN => fan_to_triangles(&sequence, &where_)?,
                    _ => sequence,
                };
                if local_indices.len() % 3 != 0 {
                    return Err(malformed(format!(
                        "{where_} has {} indices, not a multiple of 3",
                        local_indices.len()
                    )));
                }
                if let Some(bad) = local_indices.iter().find(|i| **i as usize >= vcount) {
                    return Err(malformed(format!(
                        "{where_} index {bad} is out of range for {vcount} vertices"
                    )));
                }
                if mirrored {
                    for t in local_indices.chunks_exact_mut(3) {
                        t.swap(1, 2);
                    }
                }

                let vertex_offset = positions.len();
                let base = u32::try_from(vertex_offset).map_err(|_| {
                    unsupported("merged mesh exceeds the 2^32 vertices a glTF index can address")
                })?;
                if vertex_offset
                    .checked_add(vcount)
                    .is_none_or(|t| t > u32::MAX as usize)
                {
                    return Err(unsupported(
                        "merged mesh exceeds the 2^32 vertices a glTF index can address",
                    ));
                }

                // Positions move into world space here, once, so every later
                // stage (bounds, welding, every writer) sees the model the way
                // the file describes it rather than the way one node saw it.
                positions.extend(local_positions.iter().map(|p| transform_point(&inst.world, *p)));
                match &local_normals {
                    Some(v) => {
                        any_normals = true;
                        normals.extend(v.iter().map(|n| transform_normal(&nmat, *n)));
                    }
                    // Placeholder, resolved after the merge once it is known
                    // whether any sibling primitive had normals at all.
                    None => normals.extend(std::iter::repeat_n([0.0f32; 3], vcount)),
                }
                match &local_uvs {
                    Some(v) => {
                        any_uvs = true;
                        uvs.extend_from_slice(v);
                    }
                    // Unlike normals, a missing UV is filled with (0,0) and
                    // kept: UVs are inert without a texture (textures are out of
                    // scope module-wide), so dropping every primitive's real UVs
                    // because one sibling lacked them would lose data for no
                    // gain. A wrong NORMAL, by contrast, is visible immediately.
                    None => uvs.extend(std::iter::repeat_n([0.0f32; 2], vcount)),
                }

                let index_offset = indices.len();
                indices.extend(local_indices.iter().map(|i| i + base));
                spans.push(Span {
                    vertex_offset,
                    vertex_count: vcount,
                    index_offset,
                    index_count: local_indices.len(),
                    had_normals: local_normals.is_some(),
                });

                if base_color.is_none() {
                    base_color = self.base_color_of(prim)?;
                }
                if name.is_none() {
                    name = mesh
                        .get("name")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string);
                }
            }
        }

        if indices.is_empty() {
            return Err(MeshError::Empty);
        }

        let normals = if any_normals {
            // Mixed normals are the awkward case: the IR has no per-vertex
            // "unset", so a partially-normalled array would be a lie either way.
            // Deriving the gaps from the geometry keeps every real normal the
            // file shipped and gives the rest the value a renderer would have
            // computed anyway — strictly better than zeroing them (invalid) or
            // discarding the whole set (loses the vendor's smooth shading).
            for span in spans.iter().filter(|s| !s.had_normals) {
                let verts = &positions[span.vertex_offset..span.vertex_offset + span.vertex_count];
                let local: Vec<u32> = indices
                    [span.index_offset..span.index_offset + span.index_count]
                    .iter()
                    .map(|i| i - span.vertex_offset as u32)
                    .collect();
                let filled = derived_normals(verts, &local);
                normals[span.vertex_offset..span.vertex_offset + span.vertex_count]
                    .copy_from_slice(&filled);
            }
            Some(normals)
        } else {
            None
        };

        Ok(Mesh {
            positions,
            indices,
            normals,
            uvs: if any_uvs { Some(uvs) } else { None },
            base_color,
            name,
        })
    }
}

/// Read a glTF 2.0 binary into the neutral mesh.
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let (json, bin) = split_chunks(bytes)?;
    let doc = parse_json(json)?;
    check_required_extensions(&doc)?;
    Gltf::new(doc, bin).to_mesh()
}

// ─── writer ─────────────────────────────────────────────────────────────────

fn pad_to_4(buf: &mut Vec<u8>, filler: u8) {
    while !buf.len().is_multiple_of(4) {
        buf.push(filler);
    }
}

/// Write the neutral mesh as a single-node, single-primitive GLB.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    // `super::write` validates first, but this is a public entry point and the
    // arithmetic below indexes by `positions.len()` — re-check rather than trust
    // the caller.
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let vcount = mesh.positions.len();
    if vcount > u32::MAX as usize {
        return Err(unsupported(format!("{vcount} vertices exceeds what a glTF accessor can count")));
    }

    // glTF REQUIRES min/max on a POSITION accessor, and three.js throws outright
    // without them. Computing it here also forces the finiteness check: a NaN
    // coordinate would serialise as JSON `null` and produce a file that parses
    // and then renders nothing.
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for (i, p) in mesh.positions.iter().enumerate() {
        for a in 0..3 {
            if !p[a].is_finite() {
                return Err(malformed(format!("position {i} component {a} is {}", p[a])));
            }
            min[a] = min[a].min(p[a]);
            max[a] = max[a].max(p[a]);
        }
    }

    let mut bin: Vec<u8> = Vec::with_capacity(vcount * 12 + mesh.indices.len() * 4);
    let mut views: Vec<Value> = Vec::new();
    let mut accessors: Vec<Value> = Vec::new();
    let mut attributes = serde_json::Map::new();

    // POSITION
    for p in &mesh.positions {
        for v in p {
            bin.extend_from_slice(&v.to_le_bytes());
        }
    }
    views.push(serde_json::json!({
        "buffer": 0, "byteOffset": 0, "byteLength": bin.len(), "target": 34962
    }));
    accessors.push(serde_json::json!({
        "bufferView": 0, "componentType": COMP_FLOAT, "count": vcount,
        "type": "VEC3", "min": min, "max": max
    }));
    attributes.insert("POSITION".into(), Value::from(0));

    if let Some(normals) = &mesh.normals {
        let start = bin.len();
        for n in normals {
            for v in n {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34962
        }));
        accessors.push(serde_json::json!({
            "bufferView": views.len() - 1, "componentType": COMP_FLOAT,
            "count": vcount, "type": "VEC3"
        }));
        attributes.insert("NORMAL".into(), Value::from(accessors.len() - 1));
    }

    if let Some(uvs) = &mesh.uvs {
        let start = bin.len();
        for t in uvs {
            for v in t {
                bin.extend_from_slice(&v.to_le_bytes());
            }
        }
        views.push(serde_json::json!({
            "buffer": 0, "byteOffset": start, "byteLength": bin.len() - start, "target": 34962
        }));
        accessors.push(serde_json::json!({
            "bufferView": views.len() - 1, "componentType": COMP_FLOAT,
            "count": vcount, "type": "VEC2"
        }));
        attributes.insert("TEXCOORD_0".into(), Value::from(accessors.len() - 1));
    }

    // Indices are always UNSIGNED_INT. Narrowing to UNSIGNED_SHORT when the mesh
    // has under 65536 vertices would save at most two bytes per index — a few
    // percent of a file whose vertex attributes are 20+ bytes each — in exchange
    // for a second code path, a second alignment rule (u16 views must still land
    // on a 4-byte boundary), and a size-dependent branch that only the small
    // meshes in the test suite would ever take. One always-correct path is worth
    // more than the bytes.
    let index_start = bin.len();
    for i in &mesh.indices {
        bin.extend_from_slice(&i.to_le_bytes());
    }
    views.push(serde_json::json!({
        "buffer": 0, "byteOffset": index_start, "byteLength": bin.len() - index_start,
        "target": 34963
    }));
    accessors.push(serde_json::json!({
        "bufferView": views.len() - 1, "componentType": COMP_UNSIGNED_INT,
        "count": mesh.indices.len(), "type": "SCALAR"
    }));
    let index_accessor = accessors.len() - 1;
    // BIN pads with zeros, JSON with spaces — both are spec requirements and
    // viewers do reject violations.
    pad_to_4(&mut bin, 0x00);

    let mut primitive = serde_json::Map::new();
    primitive.insert("attributes".into(), Value::Object(attributes));
    primitive.insert("indices".into(), Value::from(index_accessor));

    let material = match mesh.base_color {
        None => None,
        Some(c) => {
            for (i, v) in c.iter().enumerate() {
                if !v.is_finite() {
                    return Err(malformed(format!("base_color[{i}] is {v}")));
                }
            }
            primitive.insert("material".into(), Value::from(0));
            // Fully rough and non-metallic: the IR carries a flat colour and
            // nothing else, and those two values are the ones that make the
            // colour show up as itself rather than as a mirror.
            Some(serde_json::json!({
                "pbrMetallicRoughness": {
                    "baseColorFactor": c, "metallicFactor": 0.0, "roughnessFactor": 1.0
                }
            }))
        }
    };

    let mut gltf_mesh = serde_json::Map::new();
    gltf_mesh.insert("primitives".into(), Value::Array(vec![Value::Object(primitive)]));
    let mut node = serde_json::Map::new();
    node.insert("mesh".into(), Value::from(0));
    if let Some(n) = &mesh.name {
        // The name goes on both so it survives a round-trip through readers that
        // look at either one; ours reads the mesh's.
        gltf_mesh.insert("name".into(), Value::from(n.clone()));
        node.insert("name".into(), Value::from(n.clone()));
    }

    let mut doc = serde_json::json!({
        "asset": { "version": "2.0", "generator": "litegen mesh converter" },
        "scene": 0,
        "scenes": [ { "nodes": [0] } ],
        "nodes": [ Value::Object(node) ],
        "meshes": [ Value::Object(gltf_mesh) ],
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [ { "byteLength": bin.len() } ]
    });
    if let (Some(m), Some(obj)) = (material, doc.as_object_mut()) {
        // Inserted only when there is one: glTF forbids empty arrays, so a
        // colourless mesh must omit `materials` rather than emit `[]`, which the
        // official validator flags even though loaders shrug at it.
        obj.insert("materials".into(), Value::Array(vec![m]));
    }
    let mut json = serde_json::to_vec(&doc)
        .map_err(|e| malformed(format!("could not serialise the glTF JSON: {e}")))?;
    pad_to_4(&mut json, b' ');

    let total = HEADER_LEN + 8 + json.len() + 8 + bin.len();
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
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin);
    Ok(out)
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
    fn rejects_normalised_short_uvs_as_unsupported() {
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
    fn rejects_a_matrix_accessor_as_unsupported() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["type"] = Value::from("MAT4");
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
    fn rejects_a_sparse_accessor_as_unsupported() {
        let (mut doc, bin) = triangle_doc();
        doc["accessors"][0]["sparse"] = serde_json::json!({"count": 1});
        let d = unwrap_unsupported(read(&pack(doc, Some(&bin))).unwrap_err());
        assert!(d.contains("sparse"), "{d}");
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
