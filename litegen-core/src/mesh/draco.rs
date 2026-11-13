//! Draco-compressed glTF primitives (`KHR_draco_mesh_compression`), decoded
//! through the `draco-core` crate (mesh-gaps item 2, decision D2: pinned
//! `=2.3.3`, decoder features only, named by this file alone). glb.rs calls
//! [`decode_primitive`] and sees only this file's plain types, so another
//! decoder would replace this file and nothing else.
//!
//! What decodes: a Draco 2.2 triangle mesh, sequential or EdgeBreaker
//! (standard or valence), the bitstream the extension is written against.
//! Refused by name: a point cloud, which the extension has no use for, a
//! bitstream older than 2.2, and the legacy prediction schemes
//! (MultiParallelogram, TexCoordsDeprecated) that draco-core's decoder build
//! leaves out and no encoder since 2017 writes, each Unsupported.
//!
//! Draco can't be bounded by its input size: a constant attribute
//! entropy-codes to a few bytes whatever its count (draco-core writes 100,000
//! points in 171 bytes). So the caller's [`Limits`] are absolute, and they
//! hold before anything is sized from a count:
//!
//! - a mesh's header states its face count and its point count (a
//!   sequential mesh's points; an EdgeBreaker mesh's connectivity vertices)
//!   before any data, and this file refuses either past its limit before
//!   draco-core reads on. draco-core itself checks a point count only in a
//!   point cloud's header, and a sequential mesh's faces only after decoding
//!   their index symbols;
//! - draco-core refuses decoded attribute bytes past the bytes limit as each
//!   attribute is declared, at the width the stream types it. Its working
//!   buffers are wider for a narrow type (up to four bytes a value, its own
//!   docs say), which is why the point limit must hold first;
//! - and the decoded point and face counts are checked again here: an
//!   EdgeBreaker mesh's attribute seams may split its vertices into more
//!   points (at most three a face, which draco-core checks before sizing
//!   anything), and a stream with metadata has no counts at a place the
//!   header gives, so it is held by the bytes limit until these checks
//!   (accepted, Ruling B37a).
//!
//! What the decoder spends, measured in release (peak resident set,
//! `/usr/bin/time -l`, mesh-gaps item 2 Task 4 fix round 1, Ruling B38): it
//! decodes every attribute a stream holds, whatever the caller asks for. An
//! EdgeBreaker attribute costs about 40 bytes for each of its points where
//! the mesh has about one face a point, and about 62 where it has two, as a
//! closed mesh does; a sequential one costs a few bytes. The bytes limit
//! charges each attribute at its declared width, at least a byte a point, so
//! it is what bounds points × attributes, and [`MAX_DECODED_BYTES`] is set
//! from that product. Each stream below declares that ceiling, 24,000,000
//! bytes (48 bytes a point at about 500,000 points), as a strip (two rows,
//! about one face a point) and as a square grid (about two), read as glb.rs
//! reads it, its scene copies made:
//!
//! | layout | EdgeBreaker strip | EdgeBreaker square | sequential strip | sequential square |
//! |---|---|---|---|---|
//! | POSITION f32 × 3 and 36 one-byte attributes (37) | 753 MB | 1,140 MB | 52 MB | 71 MB |
//! | 48 one-byte attributes, decoded only | 985 MB | 1,494 MB | 51 MB | 64 MB |
//! | 192 one-byte attributes at 124,609 points, decoded only | | 1,413 MB | | 36 MB |
//! | the ten glb.rs reads: POSITION, COLOR_0, JOINTS_0..3, WEIGHTS_0..3 | 321 MB | 374 MB | 142 MB | 163 MB |
//! | POSITION, NORMAL, TEXCOORD_0 and TANGENT, f32 | 156 MB | 214 MB | 86 MB | 105 MB |
//!
//! The worst measured, over these strips and grids, is 1,494 MB, under the
//! ~1.7 GB a conversion is given. Memory grows with the faces as well as the
//! points (about 18 bytes a point and 22 a face, an attribute), so a mesh
//! with many more faces a point than a grid sits outside what was measured;
//! the faces-per-point pre-check that would hold it is open (Ruling B39,
//! parked under B41). At twice
//! the ceiling, 48,000,000, the square EdgeBreaker streams reached 2,035 MB
//! (37 attributes) and 2,974 MB (48), and at the earlier 192,000,000 a
//! 37-attribute stream of 4,000,000 points reached 5,682 MB.
//!
//! The cost: a primitive past 24,000,000 bytes of decoded attributes is
//! refused, Unsupported, naming the ceiling. That's past 500,000 points with
//! POSITION, NORMAL, TEXCOORD_0 and TANGENT (48 bytes a point), 750,000
//! without TANGENT, and 2,000,000 with POSITION alone. A stream whose
//! header declares more points or faces than the limits (one without
//! metadata, above) is refused from it in microseconds, before anything
//! decodes.
//!
//! @see <https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_draco_mesh_compression/README.md>
//! (fetched 2026-11-06 into item 2's ledger, `task4/khronos/`)

use super::MeshError;
use draco_core::{DataType, DecodeLimits, DecoderBuffer, DracoError, ErrorKind, FaceIndex, Mesh, MeshDecoder, PointAttribute, PointIndex};

/// Every Draco stream starts with these five bytes, then its version (major,
/// minor) and its geometry type.
const MAGIC: &[u8; 5] = b"DRACO";

/// The bitstream KHR_draco_mesh_compression specifies.
const VERSION: (u8, u8) = (2, 2);

/// The header's geometry type of a point cloud (a triangle mesh is 1).
const POINT_CLOUD: u8 = 0;

/// The header's encoding methods: a sequential mesh, and an EdgeBreaker one.
const SEQUENTIAL: u8 = 0;
const EDGEBREAKER: u8 = 1;

/// The header flag saying metadata follows it, at a length of its own, so
/// that nothing after it sits at a place the header alone gives.
const METADATA: u16 = 0x8000;

/// A mesh's declared face and point counts, where its header gives them
/// before any data, as LEB128 varints (bitstream 2.2), unless metadata comes
/// between: a sequential mesh's num_faces then num_points right after the 11
/// header bytes; an EdgeBreaker mesh's num_encoded_vertices then num_faces
/// after its traversal byte. `None` where they can't be read so: metadata, or
/// a stream cut short, which the decoder then refuses itself.
fn declared_counts(stream: &[u8]) -> Option<(u64, u64)> {
    let method = *stream.get(8)?;
    if u16::from_le_bytes([*stream.get(9)?, *stream.get(10)?]) & METADATA != 0 {
        return None;
    }
    let mut at = match method {
        SEQUENTIAL => 11,
        EDGEBREAKER => 12,
        _ => return None,
    };
    let mut varint = || -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *stream.get(at)?;
            at += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    };
    let (first, second) = (varint()?, varint()?);
    Some(if method == SEQUENTIAL { (first, second) } else { (second, first) })
}

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

fn unsupported(detail: impl Into<String>) -> MeshError {
    MeshError::Unsupported(detail.into())
}

/// The prediction schemes (ids 2 and 3) that draco-core compiles out without
/// its `legacy_bitstream_decode` feature, as its error messages name them.
const LEGACY_PREDICTION_SCHEMES: [&str; 2] = ["MultiParallelogram", "TexCoordsDeprecated"];

/// draco-core's refusal as the mesh library's: its limits and what it
/// declines to decode are Unsupported (the file may be fine), and anything
/// else is the stream's fault.
fn refusal(e: &DracoError, limits: Limits) -> MeshError {
    match e.kind() {
        ErrorKind::LimitExceeded => unsupported(format!(
            "the Draco stream is past this reader's limits ({} points, {} faces, and {} bytes of decoded attributes, the \
             ceiling set to bound the Draco decoder's memory on the shapes measured): {}",
            limits.max_points,
            limits.max_faces,
            limits.max_bytes,
            e.message()
        )),
        ErrorKind::UnsupportedVersion | ErrorKind::UnknownVersion | ErrorKind::BitstreamVersionUnsupported | ErrorKind::UnsupportedFeature => {
            unsupported(format!("the Draco stream uses what this reader doesn't decode: {}", e.message()))
        }
        // The legacy prediction schemes draco-core's build leaves out
        // (`legacy_bitstream_decode` off), which it reports as General,
        // naming the scheme: "<scheme> prediction is disabled". The file may
        // be fine; it is this reader that declines (Task 4 minor 5).
        _ if LEGACY_PREDICTION_SCHEMES.iter().any(|s| e.message().contains(&format!("{s} prediction is disabled"))) => {
            let scheme = LEGACY_PREDICTION_SCHEMES.iter().find(|s| e.message().contains(*s)).copied().unwrap_or("a legacy");
            unsupported(format!(
                "the Draco stream uses the {scheme} prediction scheme, a legacy scheme no encoder since 2017 writes, which this reader's Draco decoder leaves out"
            ))
        }
        _ => malformed(format!("the Draco stream doesn't decode: {}", e.message())),
    }
}

/// The decoded-bytes ceiling a caller should pass as [`Limits::max_bytes`],
/// derived from what draco-core spends (Ruling B38).
///
/// - **What draco-core spends:** it decodes every attribute in a stream,
///   requested or not, and an EdgeBreaker attribute's working memory grows
///   with the points and with the corners. Measured in release at the peak,
///   that's about 40 bytes for each point of each attribute where a mesh has
///   one face a point, and about 62 where it has two, as a closed mesh does
///   (point maps, working and portable values, traversal and prediction
///   buffers). A sequential attribute costs a few bytes.
/// - **What it charges:** its bytes ceiling counts each attribute at its
///   declared width, at least one byte a point.
/// - **So the ceiling bounds points × attributes,** and with it the decode's
///   memory on the shapes measured, strips and grids: 24,000,000 bytes
///   holds that product at 24,000,000, about 24M × 62 B ≈ 1.5 GB per
///   decode at a grid's two faces a point. A mesh with many more faces a
///   point costs more than that per point, and the faces-per-point
///   pre-check that would hold it is open (Ruling B39).
/// - **That fits one conversion's ~1.7 GB:** 48,000,000, the ruling's first
///   figure, measured up to 2,974 MB. 24M is the largest round value under
///   that with margin: the worst measured at it, over strips and grids, is
///   1,494 MB.
///
/// The measured peaks are in this module's doc.
pub const MAX_DECODED_BYTES: usize = 24_000_000;

/// Ceilings on one decode, each absolute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_points: usize,
    pub max_faces: usize,
    /// Every decoded attribute value, at the width the stream types it, in
    /// bytes.
    pub max_bytes: usize,
}

/// How a decoded attribute's components are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
}

impl ValueKind {
    /// Bytes a component.
    pub fn size(self) -> usize {
        match self {
            ValueKind::I8 | ValueKind::U8 => 1,
            ValueKind::I16 | ValueKind::U16 => 2,
            ValueKind::I32 | ValueKind::U32 | ValueKind::F32 => 4,
        }
    }
}

/// One attribute of a decoded primitive: every point's components, in point
/// order, as the stream types them once its own dequantization is done.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAttribute {
    pub kind: ValueKind,
    pub components: usize,
    /// The stream's own `normalized` flag.
    pub normalized: bool,
    /// [`count`](Self::count) × `components` values, little-endian.
    pub bytes: Vec<u8>,
}

impl DecodedAttribute {
    /// Points the attribute holds a value for.
    pub fn count(&self) -> usize {
        self.bytes.len().checked_div(self.components * self.kind.size()).unwrap_or(0)
    }
}

/// A decoded primitive: its triangles, and the attributes the caller named.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedPrimitive {
    /// Three point indices a triangle.
    pub indices: Vec<u32>,
    /// In the order the caller named them.
    pub attributes: Vec<(String, DecodedAttribute)>,
}

/// Decode one Draco stream, `stream`, into its triangles and the attributes
/// `attributes` names, each a name and the Draco unique id the extension maps
/// it to, within `limits`.
///
/// An attribute is found by its unique id alone, whatever its place in the
/// stream, and an id the stream lacks, or two names on one id, is refused
/// rather than guessed at.
pub fn decode_primitive(stream: &[u8], attributes: &[(String, u32)], limits: Limits) -> Result<DecodedPrimitive, MeshError> {
    // The header says what the stream is, so what this build doesn't decode
    // is refused by name before draco-core reads on.
    if stream.len() > 7 && stream.starts_with(MAGIC) {
        let (major, minor) = (stream[5], stream[6]);
        if (major, minor) < VERSION {
            return Err(unsupported(format!(
                "the Draco stream is bitstream {major}.{minor}, older than 2.2, the one KHR_draco_mesh_compression specifies and this reader decodes"
            )));
        }
        if stream[7] == POINT_CLOUD {
            return Err(unsupported(
                "the Draco stream is a point cloud; KHR_draco_mesh_compression compresses triangle meshes, and this reader decodes nothing else",
            ));
        }
        // draco-core checks a point count only in a point cloud's header,
        // and its bytes limit charges an attribute at its declared width, so
        // a narrow attribute admits many times the point limit; and it checks
        // a sequential mesh's faces only after decoding their index symbols.
        // The header states both counts first, and is held to the limits
        // before anything is sized from them (fix round 1, Rulings B37 and
        // B37a). An EdgeBreaker stream's count is its connectivity's
        // vertices, which attribute seams may split into more points (Box's
        // 8 into 24), at most three a face: those are checked after the
        // decode, below.
        if let Some((faces, points)) = declared_counts(stream) {
            if points > limits.max_points as u64 {
                return Err(unsupported(format!(
                    "the Draco stream declares {points} points, past this reader's limit of {}",
                    limits.max_points
                )));
            }
            if faces > limits.max_faces as u64 {
                return Err(unsupported(format!(
                    "the Draco stream declares {faces} faces, past this reader's limit of {}",
                    limits.max_faces
                )));
            }
        }
    }
    for (i, (name, id)) in attributes.iter().enumerate() {
        if let Some((first, _)) = attributes[..i].iter().find(|(_, other)| other == id) {
            return Err(malformed(format!("{first} and {name} are both Draco attribute {id}")));
        }
    }
    let ceiling = DecodeLimits::default()
        .with_max_points(limits.max_points as u64)
        .with_max_faces(limits.max_faces as u64)
        .with_max_decoded_bytes(limits.max_bytes as u64);
    let mut mesh = Mesh::new();
    MeshDecoder::new().decode(&mut DecoderBuffer::new(stream).with_limits(ceiling), &mut mesh).map_err(|e| refusal(&e, limits))?;
    // Again on what decoded: an EdgeBreaker mesh's points, which its seams
    // may have split past its declared vertices, and any stream with
    // metadata, whose header gave no counts above.
    let points = mesh.num_points();
    if points > limits.max_points {
        return Err(unsupported(format!(
            "the Draco stream decodes {points} points, past this reader's limit of {}",
            limits.max_points
        )));
    }
    let faces = mesh.num_faces();
    if faces > limits.max_faces {
        return Err(unsupported(format!("the Draco stream decodes {faces} faces, past this reader's limit of {}", limits.max_faces)));
    }
    let mut indices = Vec::with_capacity(faces * 3);
    for f in 0..faces {
        for corner in mesh.face(FaceIndex(f as u32)) {
            if corner.0 as usize >= points {
                return Err(malformed(format!("Draco face {f} names point {}, past the stream's {points}", corner.0)));
            }
            indices.push(corner.0);
        }
    }
    let found: Vec<&PointAttribute> = (0..mesh.num_attributes()).map(|i| mesh.attribute(i)).collect();
    let mut decoded = Vec::with_capacity(attributes.len());
    for (name, id) in attributes {
        let Some(attribute) = found.iter().find(|a| a.unique_id() == *id) else {
            let ids: Vec<u32> = found.iter().map(|a| a.unique_id()).collect();
            return Err(malformed(format!("{name} is Draco attribute {id}, which the stream doesn't have (its unique ids are {ids:?})")));
        };
        decoded.push((name.clone(), values(attribute, points, name, limits.max_bytes)?));
    }
    Ok(DecodedPrimitive { indices, attributes: decoded })
}

/// One point after another, each its value through the attribute's
/// point-to-value map (EdgeBreaker shares a value among the points a seam
/// doesn't split), copied as the stream stores it: little-endian, which is
/// how draco-core keeps every buffer. draco-core charged `points` of these
/// values to the bytes limit already; the copy is held to it again, so
/// nothing here is sized past it.
fn values(a: &PointAttribute, points: usize, name: &str, max_bytes: usize) -> Result<DecodedAttribute, MeshError> {
    let kind = match a.data_type() {
        DataType::Int8 => ValueKind::I8,
        DataType::Uint8 => ValueKind::U8,
        DataType::Int16 => ValueKind::I16,
        DataType::Uint16 => ValueKind::U16,
        DataType::Int32 => ValueKind::I32,
        DataType::Uint32 => ValueKind::U32,
        DataType::Float32 => ValueKind::F32,
        other => return Err(unsupported(format!("{name}'s Draco attribute holds {other:?} values, which no glTF component type holds"))),
    };
    let components = usize::from(a.num_components());
    let width = components * kind.size();
    let Ok(stride) = usize::try_from(a.byte_stride()) else {
        return Err(malformed(format!("{name}'s Draco attribute has a negative stride")));
    };
    if width == 0 || stride < width {
        return Err(malformed(format!("{name}'s Draco attribute has {components} components at a {stride}-byte stride")));
    }
    if points.checked_mul(width).is_none_or(|n| n > max_bytes) {
        return Err(unsupported(format!("{name}'s Draco attribute decodes {points} values of {width} bytes, past this reader's limit of {max_bytes} bytes")));
    }
    // Values sit `stride` apart from the buffer's start, as draco-core's own
    // `read_f32s` reads them.
    let data = a.buffer().data();
    let mut bytes = Vec::with_capacity(points * width);
    for p in 0..points {
        let v = a.mapped_index(PointIndex(p as u32)).0 as usize;
        let value = v
            .checked_mul(stride)
            .and_then(|at| data.get(at..at.checked_add(width)?))
            .filter(|_| v < a.size())
            .ok_or_else(|| malformed(format!("{name}: Draco point {p} maps to value {v}, past the attribute's {}", a.size())))?;
        bytes.extend_from_slice(value);
    }
    Ok(DecodedAttribute { kind, components, normalized: a.normalized(), bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Box (glTF-Sample-Assets 8f492390ddc7, by Cesium, CC-BY-4.0;
    /// `testdata/gltf/draco/README.md`): its glTF-Draco variant's one stream,
    /// bufferView 0, the first 118 bytes of Box.bin. Standard EdgeBreaker, 24
    /// points and 12 faces. POSITION is unique id 1 and is stored first,
    /// NORMAL unique id 0, and its 24 points share 8 position values.
    const BOX_BIN: &[u8] = include_bytes!("testdata/gltf/draco/Box/glTF-Draco/Box.bin");

    fn box_stream() -> Vec<u8> {
        BOX_BIN[..118].to_vec()
    }

    fn box_names() -> Vec<(String, u32)> {
        vec![("POSITION".to_string(), 1), ("NORMAL".to_string(), 0)]
    }

    /// Exactly what Box decodes to: 24 points, 12 faces, and two attributes
    /// of 24 × 3 floats, 576 bytes (draco-core's own count).
    const BOX_NEEDS: Limits = Limits { max_points: 24, max_faces: 12, max_bytes: 576 };

    fn floats(a: &DecodedAttribute) -> Vec<[f32; 3]> {
        assert_eq!((a.kind, a.components), (ValueKind::F32, 3));
        a.bytes.chunks_exact(12).map(|c| std::array::from_fn(|i| f32::from_le_bytes(c[4 * i..4 * i + 4].try_into().unwrap()))).collect()
    }

    fn refusal(stream: &[u8], names: &[(String, u32)], limits: Limits) -> MeshError {
        decode_primitive(stream, names, limits).expect_err("refused")
    }

    /// Each name gets the attribute whose unique id the extension gives it,
    /// whatever its place in the stream, with one value for every point
    /// through the stream's point-to-value map. The expectations are the unit
    /// cube's own geometry: every corner ±0.5 on each axis and used by three
    /// faces, every normal a unit axis used by four points, each point on the
    /// face its normal names, and each triangle flat on one face.
    #[test]
    fn box_decodes_by_unique_id_through_its_point_map() {
        let d = decode_primitive(&box_stream(), &box_names(), BOX_NEEDS).unwrap_or_else(|e| panic!("Box: {e}"));
        let names: Vec<&str> = d.attributes.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["POSITION", "NORMAL"], "in the order named");
        let positions = floats(&d.attributes[0].1);
        let normals = floats(&d.attributes[1].1);
        assert_eq!((positions.len(), normals.len()), (24, 24), "a value for every point");
        let mut corners: Vec<[f32; 3]> = positions.clone();
        corners.sort_by(|a, b| a.partial_cmp(b).unwrap());
        corners.dedup();
        assert_eq!(corners.len(), 8, "8 corners: {positions:?}");
        for c in &corners {
            assert!(c.iter().all(|v| v.abs() == 0.5), "a corner of the unit cube: {c:?}");
            assert_eq!(positions.iter().filter(|p| *p == c).count(), 3, "{c:?} is on three faces");
        }
        for (p, n) in positions.iter().zip(&normals) {
            let axis = (0..3).find(|&i| n[i] != 0.0).unwrap_or_else(|| panic!("a zero normal: {n:?}"));
            assert!(n[axis].abs() == 1.0 && (0..3).all(|i| i == axis || n[i] == 0.0), "a unit axis: {n:?}");
            assert_eq!(p[axis], 0.5 * n[axis], "point {p:?} is on the face its normal {n:?} names");
        }
        for axis in [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, -1.0]] {
            assert_eq!(normals.iter().filter(|n| **n == axis).count(), 4, "{axis:?} has four points");
        }
        assert_eq!(d.indices.len(), 36, "12 triangles");
        for t in d.indices.chunks(3) {
            assert!(t.iter().all(|&i| i < 24), "{t:?}");
            assert!(normals[t[0] as usize] == normals[t[1] as usize] && normals[t[1] as usize] == normals[t[2] as usize], "triangle {t:?} is on one face");
        }
    }

    /// Every limit reaches the decode: Box decodes under exactly what it
    /// needs, and each limit one under that refuses it as Unsupported (the
    /// file is fine; the reader declines its size), naming the ceiling.
    #[test]
    fn the_limits_reach_the_decoder() {
        let stream = box_stream();
        assert!(decode_primitive(&stream, &box_names(), BOX_NEEDS).is_ok(), "exactly Box's needs");
        for (limits, what) in [
            (Limits { max_points: 23, ..BOX_NEEDS }, "23"),
            (Limits { max_faces: 11, ..BOX_NEEDS }, "11"),
            (Limits { max_bytes: 575, ..BOX_NEEDS }, "575"),
        ] {
            match refusal(&stream, &box_names(), limits) {
                MeshError::Unsupported(m) => assert!(m.contains(what), "{limits:?}: {m}"),
                other => panic!("{limits:?}: {other:?}"),
            }
        }
    }

    /// A point cloud (geometry type 0 in the header) is refused by name: the
    /// extension compresses triangle meshes, and this build decodes nothing
    /// else (Ruling B29).
    #[test]
    fn a_point_cloud_is_refused_by_name() {
        let mut stream = box_stream();
        assert_eq!(stream[7], 1, "Box is a triangle mesh");
        stream[7] = 0;
        match refusal(&stream, &box_names(), BOX_NEEDS) {
            MeshError::Unsupported(m) => assert!(m.contains("point cloud"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// A bitstream older than 2.2 is refused by name, saying which it is: the
    /// extension specifies 2.2, and this build reads no older layout (Ruling
    /// B29).
    #[test]
    fn a_bitstream_before_2_2_is_refused_by_name() {
        assert_eq!(&box_stream()[5..7], [2, 2], "Box is bitstream 2.2");
        for (major, minor) in [(2u8, 1u8), (2, 0), (1, 3)] {
            let mut stream = box_stream();
            stream[5] = major;
            stream[6] = minor;
            match refusal(&stream, &box_names(), BOX_NEEDS) {
                MeshError::Unsupported(m) => assert!(m.contains(&format!("{major}.{minor}")) && m.contains("2.2"), "{major}.{minor}: {m}"),
                other => panic!("{major}.{minor}: {other:?}"),
            }
        }
    }

    /// draco-core 2.3.3's own encoder's sequential mesh of 1,000 points whose
    /// one attribute is GENERIC, one unsigned byte a point (the constant 7),
    /// with one face (`testdata/gltf/draco/README.md`). Its header's counts,
    /// num_faces and num_points, are the varints at bytes 11 and 12..14.
    const NARROW: &[u8] = include_bytes!("testdata/gltf/draco/crafted/narrow-u8-1000.drc");

    /// MorphPrimitivesTest's first stream (glTF-Sample-Assets 8f492390ddc7,
    /// CC-BY-4.0; its glTF-Draco .bin, bytes 360..670): sequential, 24 faces
    /// and 21 points, the varints at bytes 11 and 12.
    const MORPH_BIN: &[u8] = include_bytes!("testdata/gltf/draco/MorphPrimitivesTest/glTF-Draco/MorphPrimitivesTest.bin");

    /// A sequential stream's point count is the limit's before anything is
    /// sized from it (fix round 1, Ruling B37). draco-core checks a point
    /// count only in a point cloud's header, and an attribute one byte a
    /// point is charged one byte, so its bytes limit alone lets a narrow
    /// stream declare far more points than the point limit and decode them.
    /// The header's count is read and refused first, as Unsupported naming
    /// the limit: refused alike when every byte after the counts is cut
    /// away, so no decode reads the stream's data.
    #[test]
    fn a_narrow_stream_declaring_past_the_point_limit_is_refused_before_decoding() {
        let limits = Limits { max_points: 999, max_faces: 1, max_bytes: 1 << 20 };
        assert_eq!((NARROW[11], &NARROW[12..14]), (1, &[0xe8, 0x07][..]), "one face, 1,000 points");
        for (what, stream) in [("the stream", NARROW), ("its header alone", &NARROW[..14])] {
            match refusal(stream, &[("_NARROW".to_string(), 0)], limits) {
                MeshError::Unsupported(m) => assert!(m.contains("declares 1000 points") && m.contains("999"), "{what}: {m}"),
                other => panic!("{what}: {other:?}"),
            }
        }
        assert!(decode_primitive(NARROW, &[("_NARROW".to_string(), 0)], Limits { max_points: 1000, ..limits }).is_ok(), "at the limit it decodes");
    }

    /// An EdgeBreaker stream's declared vertex count is held to the point
    /// limit the same way (Ruling B37a): its header gives it, after the
    /// traversal byte, before the face count and any data. Box declares 8
    /// vertices (its 24 points are those 8 split along its normals' seams),
    /// so a limit of 7 refuses it from the header, the header alone too.
    #[test]
    fn an_edgebreaker_stream_declaring_past_the_point_limit_is_refused_before_decoding() {
        let stream = box_stream();
        let limits = Limits { max_points: 7, ..BOX_NEEDS };
        assert_eq!((stream[8], stream[11], stream[12], stream[13]), (1, 0, 8, 12), "EdgeBreaker, standard traversal, 8 vertices, 12 faces");
        for (what, bytes) in [("the stream", &stream[..]), ("its header alone", &stream[..14])] {
            match refusal(bytes, &box_names(), limits) {
                MeshError::Unsupported(m) => assert!(m.contains("declares 8 points") && m.contains('7'), "{what}: {m}"),
                other => panic!("{what}: {other:?}"),
            }
        }
    }

    /// The same for a sequential stream's face count, which draco-core's
    /// compressed-connectivity path checks only after decoding the index
    /// symbols: MorphPrimitivesTest's 24 faces against a limit of 23.
    #[test]
    fn a_sequential_stream_declaring_past_the_face_limit_is_refused_before_decoding() {
        let morph = &MORPH_BIN[360..670];
        let names = [("NORMAL".to_string(), 0), ("POSITION".to_string(), 1), ("TEXCOORD_0".to_string(), 2)];
        let limits = Limits { max_points: 21, max_faces: 23, max_bytes: 1 << 20 };
        assert_eq!((morph[8], morph[11], morph[12]), (0, 24, 21), "sequential, 24 faces, 21 points");
        for (what, stream) in [("the stream", morph), ("its header alone", &morph[..13])] {
            match refusal(stream, &names, limits) {
                MeshError::Unsupported(m) => assert!(m.contains("declares 24 faces") && m.contains("23"), "{what}: {m}"),
                other => panic!("{what}: {other:?}"),
            }
        }
        assert!(decode_primitive(morph, &names, Limits { max_faces: 24, ..limits }).is_ok(), "at the limit it decodes");
    }

    /// Bytes that aren't a whole Draco stream are Malformed: no magic, the
    /// wrong magic, and a stream cut short.
    #[test]
    fn a_cut_or_foreign_stream_is_malformed() {
        let mut foreign = box_stream();
        foreign[4] = b'X';
        for (what, stream) in [("empty", Vec::new()), ("DRACX", foreign), ("cut to 60 bytes", box_stream()[..60].to_vec())] {
            assert!(matches!(refusal(&stream, &box_names(), BOX_NEEDS), MeshError::Malformed(_)), "{what}");
        }
    }

    /// Google's own decoding of a corpus of our meshes, `testdata/gltf/draco/
    /// reference/` (its README; mesh-gaps item 2 Task 4b, Rulings B33, B42,
    /// B44): every triangle primitive of this module's fixtures and of the
    /// Khronos samples, with normals, UVs, colours, tangents, joints and
    /// weights, encoded by google/draco d429de84b52a's own library at
    /// draco_encoder's `-cl 0,1,5,7,10` × `-qp 8,11,14,16`, plus compressed
    /// sequential connectivity, the other EdgeBreaker coder and metadata. The
    /// manifest holds, for each stream, Google's decoder's point and face
    /// counts and the SHA-256 of its connectivity and of each attribute's
    /// values, point by point; `streams.inc` lists the streams' bytes.
    const REFERENCE_MANIFEST: &str = include_str!("testdata/gltf/draco/reference/manifest.txt");
    const REFERENCE_STREAMS: &[(&str, &[u8])] = include!("testdata/gltf/draco/reference/streams.inc");

    /// SHA-256 (FIPS 180-4), std only, so the manifest's hashes can be
    /// checked here: `mesh/` takes no crate for it.
    fn sha256(data: &[u8]) -> String {
        const K: [u32; 64] = [
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74,
            0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d,
            0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e,
            0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
            0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        ];
        let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut message = data.to_vec();
        message.push(0x80);
        while message.len() % 64 != 56 {
            message.push(0);
        }
        message.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
        for block in message.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (word, bytes) in w.iter_mut().zip(block.chunks_exact(4)) {
                *word = u32::from_be_bytes(bytes.try_into().unwrap());
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for (k, wi) in K.iter().zip(w) {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let t1 = hh.wrapping_add(s1).wrapping_add((e & f) ^ (!e & g)).wrapping_add(*k).wrapping_add(wi);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let t2 = s0.wrapping_add((a & b) ^ (a & c) ^ (b & c));
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            for (sum, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *sum = sum.wrapping_add(v);
            }
        }
        h.iter().map(|v| format!("{v:08x}")).collect()
    }

    /// One `attribute` row of the manifest.
    struct ExpectedAttribute<'a> {
        id: u32,
        kind: ValueKind,
        components: usize,
        normalized: bool,
        sha: &'a str,
        name: &'a str,
    }

    /// One `stream` row of the manifest and its attributes.
    struct ExpectedStream<'a> {
        file: &'a str,
        points: usize,
        faces: usize,
        indices_sha: &'a str,
        encoding: &'a str,
        metadata: bool,
        /// Google's decoder refuses the stream; the expectations are its
        /// decoding of this one (Ruling B44).
        expected_from: Option<&'a str>,
        attributes: Vec<ExpectedAttribute<'a>>,
    }

    /// The manifest's stream rows, and its `same` rows as (pair, stream).
    fn reference_manifest() -> (Vec<ExpectedStream<'static>>, Vec<(&'static str, &'static str)>) {
        let kind = |name: &str| match name {
            "int8" => ValueKind::I8,
            "uint8" => ValueKind::U8,
            "int16" => ValueKind::I16,
            "uint16" => ValueKind::U16,
            "int32" => ValueKind::I32,
            "uint32" => ValueKind::U32,
            "float32" => ValueKind::F32,
            other => panic!("manifest data type {other}"),
        };
        let (mut streams, mut same) = (Vec::<ExpectedStream>::new(), Vec::new());
        for line in REFERENCE_MANIFEST.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let f: Vec<&str> = line.split(' ').collect();
            match f[0] {
                "stream" => {
                    let tail = &f[6..];
                    let metadata = tail.first() == Some(&"metadata");
                    let rest = &tail[usize::from(metadata)..];
                    let expected_from = match rest {
                        [] => None,
                        ["google-refused", "expected-from", from] => Some(*from),
                        other => panic!("{}: {other:?}", f[1]),
                    };
                    streams.push(ExpectedStream { file: f[1], points: f[2].parse().unwrap(), faces: f[3].parse().unwrap(), indices_sha: f[4], encoding: f[5], metadata, expected_from, attributes: Vec::new() });
                }
                "attribute" => {
                    assert_eq!(f.len(), 8, "{line}");
                    let stream = streams.last_mut().expect("an attribute row follows its stream");
                    stream.attributes.push(ExpectedAttribute { id: f[1].parse().unwrap(), kind: kind(f[3]), components: f[4].parse().unwrap(), normalized: f[5] == "1", sha: f[6], name: f[7] });
                }
                "same" => same.push((f[1], f[2])),
                other => panic!("manifest row {other}"),
            }
        }
        (streams, same)
    }

    /// draco-core decodes every stream of the reference corpus to exactly
    /// what Google's own decoder does: the same point and face counts, the
    /// same connectivity in the same order, and the same bytes for every
    /// attribute (Ruling B33: hash equality is byte equality). A difference
    /// names the stream and the attribute; it is a decoder disagreement to
    /// report, never a tolerance to widen (Ruling B35).
    ///
    /// Two streams Google's decoder refuses (compressed sequential
    /// connectivity under three bytes a face, which its
    /// `mesh_sequential_decoder.cc:58` sanity check mistakes for a corrupt
    /// face count) are held to its decoding of the same input's `cl0-qp11`
    /// stream, shown byte-identical for every other input (Ruling B44; the
    /// README's cross-check).
    #[test]
    fn draco_core_matches_googles_decoder() {
        assert_eq!(sha256(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let thousand: Vec<u8> = (0..1000).map(|i| i as u8).collect();
        assert_eq!(sha256(&thousand), "a8af099bf2e878609558dbf69d8f88f4a31040a8cf84b549a0cfa912f12ffc3f", "a many-block message");

        let (expected, same) = reference_manifest();
        let listed: std::collections::BTreeMap<&str, &[u8]> = REFERENCE_STREAMS.iter().copied().collect();
        let rows: std::collections::BTreeSet<&str> = expected.iter().map(|s| s.file).collect();
        assert_eq!(listed.len(), REFERENCE_STREAMS.len(), "streams.inc names each stream once");
        assert_eq!(listed.keys().copied().collect::<std::collections::BTreeSet<_>>(), rows, "the manifest's streams are streams.inc's");

        // The corpus is every input at every setting: each pair is a stream
        // or a `same` row naming one, and the settings cover every coding.
        let pair_setting = |file: &str| file.rsplit('.').nth(1).unwrap().to_string();
        let pair_input = |file: &str| file.rsplitn(3, '.').nth(2).unwrap().to_string();
        let mut pairs: Vec<&str> = expected.iter().map(|s| s.file).collect();
        for (pair, stream) in &same {
            assert!(rows.contains(stream), "{pair}: `same` names a stream the manifest lacks, {stream}");
            pairs.push(pair);
        }
        pairs.sort_unstable();
        let inputs: std::collections::BTreeSet<String> = pairs.iter().map(|p| pair_input(p)).collect();
        let settings: std::collections::BTreeSet<String> = pairs.iter().map(|p| pair_setting(p)).collect();
        assert_eq!(settings.len(), 23, "5 levels × 4 position bit depths, and 3 extras: {settings:?}");
        assert_eq!(pairs.len(), inputs.len() * settings.len(), "every input at every setting, once: {} inputs", inputs.len());
        assert!(pairs.windows(2).all(|w| w[0] != w[1]), "no pair twice");
        let encodings: std::collections::BTreeSet<&str> = expected.iter().map(|s| s.encoding).collect();
        assert_eq!(encodings.into_iter().collect::<Vec<_>>(), ["edgebreaker-standard", "edgebreaker-valence", "sequential", "sequential-compressed"]);
        assert!(expected.iter().any(|s| s.metadata), "streams with metadata");
        let refused: Vec<&str> = expected.iter().filter(|s| s.expected_from.is_some()).map(|s| s.file).collect();
        assert_eq!(refused, ["fixture-morph.seqc-qp11.drc", "fixture-skinned.seqc-qp11.drc"], "the two streams Google's decoder refuses (Ruling B44)");
        // Each refused row's expectations are its substitute's, value for
        // value (Ruling B44; Task 4b minor 1, F-T4bm1): the same counts, the
        // same connectivity hash, and the same attributes in the same order.
        fn shape<'a>(r: &ExpectedStream<'a>) -> Vec<(u32, ValueKind, usize, bool, &'a str, &'a str)> {
            r.attributes.iter().map(|a| (a.id, a.kind, a.components, a.normalized, a.sha, a.name)).collect()
        }
        for s in &expected {
            if let Some(from) = s.expected_from {
                assert!(rows.contains(from), "{}: expected-from names a stream the manifest lacks, {from}", s.file);
                let substitute = expected.iter().find(|r| r.file == from).unwrap();
                assert_eq!((s.points, s.faces, s.indices_sha), (substitute.points, substitute.faces, substitute.indices_sha), "{}: its counts and connectivity are {from}'s", s.file);
                assert_eq!(shape(s), shape(substitute), "{}: its attributes are {from}'s", s.file);
            }
        }

        let limits = Limits { max_points: 1 << 20, max_faces: 1 << 20, max_bytes: MAX_DECODED_BYTES };
        let mut failures = Vec::new();
        let mut decoded_attributes = 0;
        for s in &expected {
            let names: Vec<(String, u32)> = s.attributes.iter().map(|a| (a.name.to_string(), a.id)).collect();
            let d = match decode_primitive(listed[s.file], &names, limits) {
                Ok(d) => d,
                Err(e) => {
                    failures.push(format!("{}: refused: {e}", s.file));
                    continue;
                }
            };
            if d.indices.len() != s.faces * 3 || d.indices.iter().any(|&i| i as usize >= s.points) {
                failures.push(format!("{}: {} indices, max {:?}; Google has {} faces of {} points", s.file, d.indices.len(), d.indices.iter().max(), s.faces, s.points));
            }
            let indices: Vec<u8> = d.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            if sha256(&indices) != s.indices_sha {
                failures.push(format!("{}: the connectivity differs from Google's", s.file));
            }
            for (a, (name, got)) in s.attributes.iter().zip(&d.attributes) {
                assert_eq!(name, a.name);
                let shape = (got.kind, got.components, got.normalized, got.count());
                if shape != (a.kind, a.components, a.normalized, s.points) {
                    failures.push(format!("{} {}: {shape:?}; Google has {:?}", s.file, a.name, (a.kind, a.components, a.normalized, s.points)));
                } else if sha256(&got.bytes) != a.sha {
                    failures.push(format!("{} {}: the values differ from Google's", s.file, a.name));
                }
                decoded_attributes += 1;
            }
        }
        assert!(failures.is_empty(), "{} of {} streams differ from Google's decoder:\n{}", failures.len(), expected.len(), failures.join("\n"));
        assert_eq!(decoded_attributes, expected.iter().map(|s| s.attributes.len()).sum::<usize>());
    }

    /// A unique id the stream has no attribute for is refused, naming it, and
    /// so are two names on one id: either would hand an attribute's values to
    /// the wrong semantic or to none.
    #[test]
    fn an_id_the_stream_lacks_or_two_names_share_is_refused() {
        let stream = box_stream();
        match refusal(&stream, &[("NORMAL".to_string(), 7)], BOX_NEEDS) {
            MeshError::Malformed(m) => assert!(m.contains("NORMAL") && m.contains('7'), "{m}"),
            other => panic!("{other:?}"),
        }
        match refusal(&stream, &[("POSITION".to_string(), 1), ("NORMAL".to_string(), 1)], BOX_NEEDS) {
            MeshError::Malformed(m) => assert!(m.contains("POSITION") && m.contains("NORMAL"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    /// Where Box's stream states POSITION's prediction scheme: the
    /// `prediction_scheme_method` byte of the first attribute's data
    /// (draco-core `sequential_integer_attribute_decoder.rs`, read before
    /// the transform type byte and the symbols). Found by setting each byte
    /// of the stream to a legacy id and watching which ones draco-core
    /// names (F-T4m5's probe): 52, POSITION's, 0x01 (parallelogram), and 95,
    /// NORMAL's, 0x06 (geometric normal). The test below holds 52 to that.
    const BOX_POSITION_PREDICTION_SCHEME_BYTE: usize = 52;

    /// Task 4 minors 4 and 5 (F-T4m5): draco-core's build leaves out the
    /// legacy prediction schemes, MultiParallelogram (id 2) and
    /// TexCoordsDeprecated (id 3), which no encoder since 2017 writes. A
    /// stream that declares one is Unsupported, naming the scheme: the file
    /// may be fine, it is this reader that declines. Every other id keeps
    /// what it gives today: the stream's own (1, parallelogram) decodes, and
    /// so does Difference (0), the same symbols under another predictor;
    /// ConstrainedMultiParallelogram (4) reads per-corner selection bits the
    /// stream doesn't hold, and the portable texture-coordinate (5) and
    /// geometric normal (6) schemes need a parent attribute POSITION has
    /// none of: each Malformed; 7 is no scheme at all, which draco-core
    /// reports as an unsupported feature ("Prediction method 7"), so it is
    /// Unsupported.
    #[test]
    fn a_legacy_prediction_scheme_is_unsupported_by_name() {
        let stream = box_stream();
        let at = BOX_POSITION_PREDICTION_SCHEME_BYTE;
        assert_eq!(stream[at], 1, "Box's POSITION is parallelogram-predicted");
        let with = |id: u8| {
            let mut s = stream.clone();
            s[at] = id;
            decode_primitive(&s, &box_names(), BOX_NEEDS)
        };
        for (id, name) in [(2u8, "MultiParallelogram"), (3, "TexCoordsDeprecated")] {
            match with(id) {
                Err(MeshError::Unsupported(m)) => assert!(m.contains(name) && m.contains("prediction"), "id {id}: {m}"),
                other => panic!("id {id} ({name}): {other:?}"),
            }
        }
        assert!(with(1).is_ok(), "the stream's own scheme");
        assert!(with(0).is_ok(), "id 0: the same symbols under the difference predictor decode");
        for id in [4u8, 5, 6] {
            assert!(matches!(with(id), Err(MeshError::Malformed(_))), "id {id}: {:?}", with(id));
        }
        match with(7) {
            Err(MeshError::Unsupported(m)) => assert!(m.contains("Prediction method 7"), "{m}"),
            other => panic!("id 7: {other:?}"),
        }
    }
}
