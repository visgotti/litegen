//! The scene every reader produces and every writer consumes.
//!
//! # Why a scene and not a mesh
//!
//! The first version of this module converted one merged, world-space triangle
//! mesh, and said so: no materials, no textures, no hierarchy, nothing that
//! moves. That was enough to turn a GLB into an STL. It is not enough to turn a
//! GLB into an FBX someone can drop into Unity, or a USDZ that opens in AR Quick
//! Look with its textures on — which is what the containers that carry more than
//! geometry exist for. So the hub is now shaped like glTF's own document model,
//! because glTF is the one container every vendor emits and the one designed as
//! an interchange format: nodes with transforms, meshes made of primitives,
//! metallic-roughness materials with embedded images, skins with inverse bind
//! matrices, morph targets, and keyframed animation of node transforms and
//! morph weights.
//!
//! The old flat [`super::ir::Mesh`] remains, now as a *view* of a scene
//! ([`super::pose::flatten`]) for the containers that hold nothing else (STL)
//! and for callers that only want geometry.
//!
//! # Conventions
//!
//! Everything in a [`Scene`] is in glTF's conventions, whatever container it came
//! from; each codec converts at its own boundary and nowhere else:
//!
//! - right-handed, +Y up, metres;
//! - UV origin at the TOP-left of the image, V running down;
//! - rotations as unit quaternions `[x, y, z, w]`;
//! - colour factors and vertex colours in LINEAR space, images left encoded
//!   exactly as they arrived (PNG/JPEG bytes are never decoded or re-encoded);
//! - triangles wound counter-clockwise when seen from the front;
//! - a skinned primitive is evaluated as `Σ wᵢ · world(jointᵢ) · inverseBindᵢ`,
//!   ignoring the transform of the node that holds it, exactly as glTF
//!   specifies — FBX's cluster matrices, USD's bind transforms and Collada's
//!   bind-shape matrix are all translated into that one form on read.
//!
//! # Lossiness is reported, never silent
//!
//! [`Capabilities`] says what each container can hold, and [`losses`] compares a
//! scene against it. A writer that drops an animation because its target cannot
//! hold one is doing its job; a writer that drops it without saying so is the
//! kind of quiet wrongness this module exists to avoid.

use super::math::{self, Mat4};
use super::MeshError;

/// Ceiling on the vertices summed over every primitive in a scene.
///
/// The same bound the flat reader has always enforced (see `glb.rs`), applied
/// to the scene as a whole: 4M vertices is what a 128 MB GLB can honestly carry,
/// and instancing must not be able to amplify a small file past it.
pub const MAX_SCENE_VERTICES: usize = 4_000_000;
pub const MAX_SCENE_INDICES: usize = 12_000_000;
/// Caps on structural counts. Each is far beyond any real asset; together they
/// stop a hostile file from describing a few million empty nodes, channels or
/// keyframes that each cost an allocation.
pub const MAX_NODES: usize = 100_000;
pub const MAX_ANIMATION_KEYS: usize = 20_000_000;
pub const MAX_JOINTS: usize = 65_535;
pub const MAX_MORPH_TARGETS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scene {
    pub name: Option<String>,
    pub nodes: Vec<Node>,
    /// The nodes with no parent, in display order.
    pub roots: Vec<usize>,
    pub meshes: Vec<SceneMesh>,
    pub materials: Vec<Material>,
    pub textures: Vec<Texture>,
    pub images: Vec<Image>,
    pub skins: Vec<Skin>,
    pub animations: Vec<Animation>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    Trs { translation: [f64; 3], rotation: [f64; 4], scale: [f64; 3] },
    /// Column-major. Kept as a matrix only when it does not decompose (shear);
    /// readers decompose everything else so animation, which needs TRS, can
    /// address any node.
    Matrix(Mat4),
}

impl Default for Transform {
    fn default() -> Self {
        Transform::Trs { translation: [0.0; 3], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] }
    }
}

impl Transform {
    pub fn matrix(&self) -> Mat4 {
        match self {
            Transform::Trs { translation, rotation, scale } => math::compose(*translation, *rotation, *scale),
            Transform::Matrix(m) => *m,
        }
    }

    /// The TRS form, or `None` for a sheared matrix that has none.
    pub fn trs(&self) -> Option<([f64; 3], [f64; 4], [f64; 3])> {
        match self {
            Transform::Trs { translation, rotation, scale } => Some((*translation, *rotation, *scale)),
            Transform::Matrix(m) => math::decompose(m),
        }
    }

    /// Build from a matrix, preferring TRS whenever the matrix has one.
    pub fn from_matrix(m: Mat4) -> Transform {
        match math::decompose(&m) {
            Some((translation, rotation, scale)) => Transform::Trs { translation, rotation, scale },
            None => Transform::Matrix(m),
        }
    }

    pub fn is_identity(&self) -> bool {
        let m = self.matrix();
        m.iter().zip(math::IDENTITY.iter()).all(|(a, b)| (a - b).abs() < 1e-12)
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Node {
    pub name: Option<String>,
    pub transform: Transform,
    pub children: Vec<usize>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    /// Morph weights for this instance, overriding the mesh's defaults.
    pub weights: Option<Vec<f32>>,
}

/// glTF's `mesh`: the primitives drawn together by one node.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneMesh {
    pub name: Option<String>,
    pub primitives: Vec<Primitive>,
    /// Default morph weights; one per target, or empty when there are none.
    pub weights: Vec<f32>,
    /// Names for the morph targets, the de-facto `extras.targetNames`
    /// convention in glTF and the channel names in FBX/USD/Collada.
    pub target_names: Vec<String>,
}

impl SceneMesh {
    pub fn target_count(&self) -> usize {
        self.primitives.first().map_or(0, |p| p.targets.len())
    }
}

/// One draw: a triangle list with one material.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Primitive {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    /// xyz tangent plus handedness in w, as glTF's TANGENT.
    pub tangents: Option<Vec<[f32; 4]>>,
    /// Texture coordinate sets, TEXCOORD_0 first. Each is `positions.len()`
    /// long. glTF convention: V runs DOWN the image.
    pub uvs: Vec<Vec<[f32; 2]>>,
    /// COLOR_0, linear RGBA.
    pub colors: Option<Vec<[f32; 4]>>,
    pub skin: Option<SkinWeights>,
    /// Triangle list. Strips and fans are expanded on read.
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    pub targets: Vec<MorphTarget>,
}

impl Primitive {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Per-vertex joint influences, `influences` per vertex, stored flat.
///
/// Flat rather than glTF's fixed sets of four because FBX, USD and Collada all
/// allow any number of influences, and squeezing them through four would be a
/// lossy step on the way in that nothing asked for. `influences` is always a
/// multiple of four so the glTF writer can split it into JOINTS_n/WEIGHTS_n
/// sets without padding logic of its own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkinWeights {
    pub influences: usize,
    /// Indices into the owning node's `Skin::joints`, not node indices.
    pub joints: Vec<u16>,
    pub weights: Vec<f32>,
}

impl SkinWeights {
    pub fn vertex(&self, v: usize) -> (&[u16], &[f32]) {
        let s = v * self.influences;
        (&self.joints[s..s + self.influences], &self.weights[s..s + self.influences])
    }
}

/// A morph target: per-vertex OFFSETS added at the target's weight.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MorphTarget {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlphaMode {
    #[default]
    Opaque,
    Mask,
    Blend,
}

/// glTF's metallic-roughness material, with the extensions that change what a
/// surface looks like and that other containers can express.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: Option<String>,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub base_color_texture: Option<TextureRef>,
    pub metallic: f32,
    pub roughness: f32,
    /// glTF packing: roughness in G, metalness in B.
    pub metallic_roughness_texture: Option<TextureRef>,
    pub normal_texture: Option<TextureRef>,
    pub normal_scale: f32,
    pub occlusion_texture: Option<TextureRef>,
    pub occlusion_strength: f32,
    /// Linear RGB, already multiplied by `KHR_materials_emissive_strength`.
    pub emissive: [f32; 3],
    pub emissive_texture: Option<TextureRef>,
    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    /// `KHR_materials_unlit`: the base colour is the final colour.
    pub unlit: bool,
}

impl Default for Material {
    fn default() -> Self {
        // glTF's own defaults, so a material read from a file that omits a
        // field means what the spec says it means.
        Material {
            name: None,
            base_color: [1.0; 4],
            base_color_texture: None,
            metallic: 1.0,
            roughness: 1.0,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion_strength: 1.0,
            emissive: [0.0; 3],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            unlit: false,
        }
    }
}

impl Material {
    pub fn texture_refs(&self) -> impl Iterator<Item = (TextureSlot, &TextureRef)> {
        [
            (TextureSlot::BaseColor, self.base_color_texture.as_ref()),
            (TextureSlot::MetallicRoughness, self.metallic_roughness_texture.as_ref()),
            (TextureSlot::Normal, self.normal_texture.as_ref()),
            (TextureSlot::Occlusion, self.occlusion_texture.as_ref()),
            (TextureSlot::Emissive, self.emissive_texture.as_ref()),
        ]
        .into_iter()
        .filter_map(|(slot, r)| r.map(|r| (slot, r)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureSlot {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
}

impl TextureSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            TextureSlot::BaseColor => "baseColor",
            TextureSlot::MetallicRoughness => "metallicRoughness",
            TextureSlot::Normal => "normal",
            TextureSlot::Occlusion => "occlusion",
            TextureSlot::Emissive => "emissive",
        }
    }

    /// Colour textures are sRGB-encoded; data textures are linear. The
    /// distinction matters to every writer that has to label it (USD's
    /// `sourceColorSpace`, 3MF's colour handling).
    pub fn is_color(self) -> bool {
        matches!(self, TextureSlot::BaseColor | TextureSlot::Emissive)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextureRef {
    pub texture: usize,
    /// Which `Primitive::uvs` set to sample.
    pub uv_set: u32,
    pub transform: Option<UvTransform>,
}

/// `KHR_texture_transform`: `uv' = T(offset) * R(rotation) * S(scale) * uv`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvTransform {
    pub offset: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
}

impl UvTransform {
    pub fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        let (s, c) = self.rotation.sin_cos();
        let x = uv[0] * self.scale[0];
        let y = uv[1] * self.scale[1];
        [c * x + s * y + self.offset[0], -s * x + c * y + self.offset[1]]
    }

    pub fn is_identity(&self) -> bool {
        self.offset == [0.0, 0.0] && self.rotation == 0.0 && self.scale == [1.0, 1.0]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wrap {
    #[default]
    Repeat,
    ClampToEdge,
    MirroredRepeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    Nearest,
    #[default]
    Linear,
}

/// glTF's sampler. `min_filter: None` means "unspecified" (the reader's
/// default, mipmapped trilinear in practice); otherwise `mipmaps` says whether
/// minification uses mipmaps and `mipmap_filter` how it blends between them —
/// together the six GL minification modes (`NEAREST_MIPMAP_LINEAR` is
/// `Some(Nearest)` + mipmaps + `Linear`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sampler {
    pub wrap_s: Wrap,
    pub wrap_t: Wrap,
    pub mag_filter: Option<Filter>,
    pub min_filter: Option<Filter>,
    pub mipmaps: bool,
    pub mipmap_filter: Filter,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub name: Option<String>,
    pub image: usize,
    pub sampler: Sampler,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
    Ktx2,
}

impl ImageFormat {
    /// Identify encoded image bytes by their magic. The declared MIME type of
    /// the container is not trusted: vendors label JPEGs as PNGs often enough
    /// that a viewer which believed them would fail to decode the texture.
    pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
        if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
            Some(ImageFormat::Png)
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(ImageFormat::Jpeg)
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(ImageFormat::Webp)
        } else if bytes.starts_with(&[0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB]) {
            Some(ImageFormat::Ktx2)
        } else {
            None
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Ktx2 => "image/ktx2",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Ktx2 => "ktx2",
        }
    }

    /// PNG and JPEG are the two every container and every consumer accepts;
    /// USDZ, 3MF, FBX and Collada consumers are not required to read anything
    /// else, so a WebP or KTX2 texture cannot be carried into them without a
    /// transcode this module does not do.
    pub fn is_universal(self) -> bool {
        matches!(self, ImageFormat::Png | ImageFormat::Jpeg)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub name: Option<String>,
    pub format: ImageFormat,
    /// The encoded file, byte for byte.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Skin {
    pub name: Option<String>,
    /// Node indices, in the order `SkinWeights::joints` refers to them.
    pub joints: Vec<usize>,
    /// One per joint, column-major. Identity when the source carried none.
    pub inverse_bind_matrices: Vec<Mat4>,
    pub skeleton: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimPath {
    Translation,
    Rotation,
    Scale,
    Weights,
}

impl AnimPath {
    pub fn as_str(self) -> &'static str {
        match self {
            AnimPath::Translation => "translation",
            AnimPath::Rotation => "rotation",
            AnimPath::Scale => "scale",
            AnimPath::Weights => "weights",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interpolation {
    #[default]
    Linear,
    Step,
    /// glTF's cubic Hermite spline: every key stores in-tangent, value,
    /// out-tangent.
    CubicSpline,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Animation {
    pub name: Option<String>,
    pub channels: Vec<Channel>,
}

impl Animation {
    /// `(start, end)` over every channel, or `None` when there are no keys.
    pub fn time_range(&self) -> Option<(f32, f32)> {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for c in &self.channels {
            if let (Some(a), Some(b)) = (c.times.first(), c.times.last()) {
                lo = lo.min(*a);
                hi = hi.max(*b);
            }
        }
        (lo <= hi).then_some((lo, hi))
    }
}

/// One animated property of one node, with its own keyframes.
///
/// glTF shares samplers between channels; the IR gives every channel its own
/// copy because no other container shares them and a shared sampler is an
/// aliasing hazard for any pass that edits keys.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub path: AnimPath,
    pub interpolation: Interpolation,
    /// Seconds, strictly increasing.
    pub times: Vec<f32>,
    /// Flat: [`Channel::stride`] values per key, three times that per key for
    /// cubic splines (in-tangent, value, out-tangent).
    pub values: Vec<f32>,
}

impl Channel {
    /// Floats per value: 3 for translation/scale, 4 for a rotation quaternion,
    /// one per morph target for weights.
    pub fn stride(&self, target_count: usize) -> usize {
        match self.path {
            AnimPath::Translation | AnimPath::Scale => 3,
            AnimPath::Rotation => 4,
            AnimPath::Weights => target_count,
        }
    }
}

// ─── validation ─────────────────────────────────────────────────────────────

fn malformed(detail: impl Into<String>) -> MeshError {
    MeshError::Malformed(detail.into())
}

fn finite3(v: &[f32; 3]) -> bool {
    v.iter().all(|c| c.is_finite())
}

impl Scene {
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.triangle_count()).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.vertex_count()).sum()
    }

    /// Every invariant a writer is entitled to assume.
    ///
    /// Readers call this before returning and writers call it before
    /// writing, so a reader bug cannot reach a writer as an out-of-bounds index
    /// on the poll path — the failure is attributed to the input instead.
    pub fn validate(&self) -> Result<(), MeshError> {
        if self.nodes.len() > MAX_NODES {
            return Err(MeshError::Unsupported(format!(
                "{} nodes exceeds the {MAX_NODES}-node limit",
                self.nodes.len()
            )));
        }
        let n_nodes = self.nodes.len();
        let (mut verts, mut idx) = (0usize, 0usize);

        // Node graph: a forest, rooted exactly at `roots`.
        let mut parent: Vec<Option<usize>> = vec![None; n_nodes];
        for (i, node) in self.nodes.iter().enumerate() {
            for &c in &node.children {
                if c >= n_nodes {
                    return Err(malformed(format!("node {i} child {c} is out of range")));
                }
                if c == i || parent[c].is_some() {
                    return Err(malformed(format!(
                        "node {c} has more than one parent; the node graph is not a tree"
                    )));
                }
                parent[c] = Some(i);
            }
            if let Some(m) = node.mesh {
                if m >= self.meshes.len() {
                    return Err(malformed(format!("node {i} mesh {m} is out of range")));
                }
            }
            if let Some(s) = node.skin {
                if s >= self.skins.len() {
                    return Err(malformed(format!("node {i} skin {s} is out of range")));
                }
                if node.mesh.is_none() {
                    return Err(malformed(format!("node {i} has a skin but no mesh")));
                }
            }
            if let Some(w) = &node.weights {
                let expected = node.mesh.map_or(0, |m| self.meshes[m].target_count());
                if w.len() != expected {
                    return Err(malformed(format!(
                        "node {i} has {} morph weights for {expected} targets",
                        w.len()
                    )));
                }
            }
            match &node.transform {
                Transform::Trs { translation, rotation, scale } => {
                    if !translation.iter().chain(rotation).chain(scale).all(|v| v.is_finite()) {
                        return Err(malformed(format!("node {i} transform is not finite")));
                    }
                }
                Transform::Matrix(m) => {
                    if !m.iter().all(|v| v.is_finite()) {
                        return Err(malformed(format!("node {i} matrix is not finite")));
                    }
                }
            }
        }
        let mut seen_root = vec![false; n_nodes];
        for &r in &self.roots {
            if r >= n_nodes {
                return Err(malformed(format!("root {r} is out of range")));
            }
            if parent[r].is_some() || std::mem::replace(&mut seen_root[r], true) {
                return Err(malformed(format!("root {r} is also a child, or listed twice")));
            }
        }
        // Cycles: a node set where everyone has a parent never reaches a root.
        // Walk up from every node with a bounded step count.
        for start in 0..n_nodes {
            let mut cur = start;
            let mut steps = 0usize;
            while let Some(p) = parent[cur] {
                cur = p;
                steps += 1;
                if steps > n_nodes {
                    return Err(malformed(format!("node {start} is part of a cycle")));
                }
            }
        }

        for (mi, mesh) in self.meshes.iter().enumerate() {
            let targets = mesh.target_count();
            if targets > MAX_MORPH_TARGETS {
                return Err(MeshError::Unsupported(format!(
                    "mesh {mi} has {targets} morph targets, over the {MAX_MORPH_TARGETS} limit"
                )));
            }
            if !mesh.weights.is_empty() && mesh.weights.len() != targets {
                return Err(malformed(format!(
                    "mesh {mi} has {} default weights for {targets} targets",
                    mesh.weights.len()
                )));
            }
            if !mesh.target_names.is_empty() && mesh.target_names.len() != targets {
                return Err(malformed(format!(
                    "mesh {mi} names {} targets but has {targets}",
                    mesh.target_names.len()
                )));
            }
            for (pi, p) in mesh.primitives.iter().enumerate() {
                let at = format!("mesh {mi} primitive {pi}");
                let n = p.positions.len();
                verts = verts.saturating_add(n);
                idx = idx.saturating_add(p.indices.len());
                if p.targets.len() != targets {
                    return Err(malformed(format!(
                        "{at} has {} morph targets where its siblings have {targets}",
                        p.targets.len()
                    )));
                }
                if let Some((i, v)) = p.positions.iter().enumerate().find(|(_, v)| !finite3(v)) {
                    return Err(malformed(format!("{at} position {i} is not finite ({v:?})")));
                }
                if !p.indices.len().is_multiple_of(3) {
                    return Err(malformed(format!(
                        "{at} index count {} is not a multiple of 3",
                        p.indices.len()
                    )));
                }
                if let Some(bad) = p.indices.iter().find(|i| **i as usize >= n) {
                    return Err(malformed(format!("{at} index {bad} out of range for {n} vertices")));
                }
                let len_ok = |len: usize, what: &str| -> Result<(), MeshError> {
                    if len != n {
                        Err(malformed(format!("{at} has {len} {what} for {n} positions")))
                    } else {
                        Ok(())
                    }
                };
                if let Some(v) = &p.normals {
                    len_ok(v.len(), "normals")?;
                    if v.iter().any(|n| !finite3(n)) {
                        return Err(malformed(format!("{at} has a non-finite normal")));
                    }
                }
                if let Some(v) = &p.tangents {
                    len_ok(v.len(), "tangents")?;
                }
                for (si, set) in p.uvs.iter().enumerate() {
                    len_ok(set.len(), &format!("uv set {si} entries"))?;
                    if set.iter().any(|t| !(t[0].is_finite() && t[1].is_finite())) {
                        return Err(malformed(format!("{at} uv set {si} is not finite")));
                    }
                }
                if let Some(v) = &p.colors {
                    len_ok(v.len(), "colours")?;
                    if v.iter().any(|c| !c.iter().all(|x| x.is_finite())) {
                        return Err(malformed(format!("{at} has a non-finite colour")));
                    }
                }
                if let Some(s) = &p.skin {
                    if s.influences == 0 || !s.influences.is_multiple_of(4) {
                        return Err(malformed(format!(
                            "{at} skin has {} influences per vertex; it must be a positive multiple of 4",
                            s.influences
                        )));
                    }
                    let want = n.checked_mul(s.influences).ok_or_else(|| malformed("skin size overflows"))?;
                    if s.joints.len() != want || s.weights.len() != want {
                        return Err(malformed(format!(
                            "{at} skin arrays are {}/{} long, expected {want}",
                            s.joints.len(),
                            s.weights.len()
                        )));
                    }
                    if s.weights.iter().any(|w| !w.is_finite()) {
                        return Err(malformed(format!("{at} has a non-finite skin weight")));
                    }
                }
                if let Some(m) = p.material {
                    if m >= self.materials.len() {
                        return Err(malformed(format!("{at} material {m} is out of range")));
                    }
                }
                for (ti, t) in p.targets.iter().enumerate() {
                    len_ok(t.positions.len(), &format!("target {ti} position offsets"))?;
                    if t.positions.iter().any(|v| !finite3(v)) {
                        return Err(malformed(format!("{at} target {ti} is not finite")));
                    }
                    if let Some(nv) = &t.normals {
                        len_ok(nv.len(), &format!("target {ti} normal offsets"))?;
                    }
                }
            }
        }
        if verts > MAX_SCENE_VERTICES {
            return Err(MeshError::Unsupported(format!(
                "the scene has {verts} vertices, over the {MAX_SCENE_VERTICES} limit"
            )));
        }
        if idx > MAX_SCENE_INDICES {
            return Err(MeshError::Unsupported(format!(
                "the scene has {idx} indices, over the {MAX_SCENE_INDICES} limit"
            )));
        }

        // Skinned primitives must point at joints their node's skin has.
        for (i, node) in self.nodes.iter().enumerate() {
            let Some(m) = node.mesh else { continue };
            let joints = node.skin.map(|s| self.skins[s].joints.len());
            for (pi, p) in self.meshes[m].primitives.iter().enumerate() {
                if let (Some(sw), Some(count)) = (&p.skin, joints) {
                    if let Some(bad) = sw.joints.iter().find(|j| **j as usize >= count) {
                        return Err(malformed(format!(
                            "node {i} mesh {m} primitive {pi} uses joint {bad} of a {count}-joint skin"
                        )));
                    }
                }
            }
        }

        for (mi, mat) in self.materials.iter().enumerate() {
            for (slot, r) in mat.texture_refs() {
                if r.texture >= self.textures.len() {
                    return Err(malformed(format!(
                        "material {mi} {} texture {} is out of range",
                        slot.as_str(),
                        r.texture
                    )));
                }
            }
            let fin = mat.base_color.iter().chain(&mat.emissive).all(|v| v.is_finite())
                && [mat.metallic, mat.roughness, mat.normal_scale, mat.occlusion_strength, mat.alpha_cutoff]
                    .iter()
                    .all(|v| v.is_finite());
            if !fin {
                return Err(malformed(format!("material {mi} has a non-finite factor")));
            }
        }
        for (ti, t) in self.textures.iter().enumerate() {
            if t.image >= self.images.len() {
                return Err(malformed(format!("texture {ti} image {} is out of range", t.image)));
            }
        }

        for (si, skin) in self.skins.iter().enumerate() {
            if skin.joints.len() > MAX_JOINTS {
                return Err(MeshError::Unsupported(format!(
                    "skin {si} has {} joints, over the {MAX_JOINTS} limit",
                    skin.joints.len()
                )));
            }
            if skin.inverse_bind_matrices.len() != skin.joints.len() {
                return Err(malformed(format!(
                    "skin {si} has {} inverse bind matrices for {} joints",
                    skin.inverse_bind_matrices.len(),
                    skin.joints.len()
                )));
            }
            if let Some(bad) = skin.joints.iter().find(|j| **j >= n_nodes) {
                return Err(malformed(format!("skin {si} joint {bad} is out of range")));
            }
            if skin.skeleton.is_some_and(|s| s >= n_nodes) {
                return Err(malformed(format!("skin {si} skeleton is out of range")));
            }
            if skin.inverse_bind_matrices.iter().flatten().any(|v| !v.is_finite()) {
                return Err(malformed(format!("skin {si} has a non-finite inverse bind matrix")));
            }
        }

        let mut keys = 0usize;
        for (ai, anim) in self.animations.iter().enumerate() {
            for (ci, ch) in anim.channels.iter().enumerate() {
                let at = format!("animation {ai} channel {ci}");
                if ch.node >= n_nodes {
                    return Err(malformed(format!("{at} targets node {} which does not exist", ch.node)));
                }
                let targets = self.nodes[ch.node].mesh.map_or(0, |m| self.meshes[m].target_count());
                if ch.path == AnimPath::Weights && targets == 0 {
                    return Err(malformed(format!(
                        "{at} animates morph weights on a node with no morph targets"
                    )));
                }
                if ch.times.is_empty() {
                    return Err(malformed(format!("{at} has no keyframes")));
                }
                keys = keys.saturating_add(ch.times.len());
                if ch.times.iter().any(|t| !t.is_finite()) || ch.times.windows(2).any(|w| w[1] <= w[0]) {
                    return Err(malformed(format!("{at} key times are not strictly increasing")));
                }
                let stride = ch.stride(targets);
                let per_key = if ch.interpolation == Interpolation::CubicSpline { stride * 3 } else { stride };
                if ch.values.len() != ch.times.len() * per_key {
                    return Err(malformed(format!(
                        "{at} has {} values for {} keys of {per_key}",
                        ch.values.len(),
                        ch.times.len()
                    )));
                }
                if ch.values.iter().any(|v| !v.is_finite()) {
                    return Err(malformed(format!("{at} has a non-finite value")));
                }
            }
        }
        if keys > MAX_ANIMATION_KEYS {
            return Err(MeshError::Unsupported(format!(
                "{keys} animation keys exceeds the {MAX_ANIMATION_KEYS} limit"
            )));
        }
        Ok(())
    }

    /// Parent of every node, `None` for roots.
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut parent = vec![None; self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            for &c in &n.children {
                if let Some(slot) = parent.get_mut(c) {
                    *slot = Some(i);
                }
            }
        }
        parent
    }

    /// Nodes in an order where every parent precedes its children.
    pub fn topological_order(&self) -> Vec<usize> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(i) = stack.pop() {
            out.push(i);
            if let Some(n) = self.nodes.get(i) {
                stack.extend(n.children.iter().rev().copied());
            }
        }
        out
    }

    /// World matrices for the default (un-animated) pose.
    pub fn world_matrices(&self) -> Vec<Mat4> {
        let locals: Vec<Mat4> = self.nodes.iter().map(|n| n.transform.matrix()).collect();
        world_from_locals(self, &locals)
    }

    /// Whether any node is a joint of some skin.
    pub fn joint_nodes(&self) -> Vec<bool> {
        let mut out = vec![false; self.nodes.len()];
        for s in &self.skins {
            for &j in &s.joints {
                if let Some(slot) = out.get_mut(j) {
                    *slot = true;
                }
            }
        }
        out
    }

    /// Append a scene node holding `mesh` under no parent.
    pub fn add_root(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        let i = self.nodes.len() - 1;
        self.roots.push(i);
        i
    }
}

/// World matrices from per-node local matrices (default or animated pose).
pub fn world_from_locals(scene: &Scene, locals: &[Mat4]) -> Vec<Mat4> {
    let mut world = vec![math::IDENTITY; scene.nodes.len()];
    let mut stack: Vec<(usize, Mat4)> = scene.roots.iter().rev().map(|&r| (r, math::IDENTITY)).collect();
    let mut steps = 0usize;
    while let Some((i, parent)) = stack.pop() {
        steps += 1;
        // `validate` has proven the graph is a forest; the step bound is what
        // keeps an unvalidated scene from looping here instead of failing.
        if steps > scene.nodes.len() {
            break;
        }
        let Some(local) = locals.get(i) else { continue };
        let w = math::mat_mul(&parent, local);
        world[i] = w;
        for &c in scene.nodes[i].children.iter().rev() {
            stack.push((c, w));
        }
    }
    world
}

// ─── capabilities and loss reporting ────────────────────────────────────────

/// How much of a material a container can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaterialSupport {
    /// No materials at all (STL).
    None,
    /// No materials, but per-vertex colour: a material's base colour is baked
    /// into the colour of every vertex it covers (PLY).
    VertexColor,
    /// Flat colours per material, and colour textures without a surface model
    /// (3MF's base materials and texture groups).
    ColorOnly,
    /// A diffuse/specular/emissive model with textures (OBJ+MTL, FBX, Collada).
    Phong,
    /// Metallic-roughness PBR with textures (glTF, USD).
    Pbr,
}

/// What a container can hold. One row of the lossiness table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    /// Separate nodes with their own transforms (otherwise geometry is baked
    /// into world space).
    pub hierarchy: bool,
    pub normals: bool,
    /// How many UV sets survive.
    pub uv_sets: usize,
    pub vertex_colors: bool,
    pub materials: MaterialSupport,
    pub textures: bool,
    pub skinning: bool,
    pub morph_targets: bool,
    pub animation: bool,
    /// More than one primitive survives as separate draws (otherwise
    /// everything is merged into one mesh).
    pub multiple_meshes: bool,
    /// Texture sampling filters (nearest/linear, mipmap mode) survive.
    /// Otherwise readers apply their defaults — which is a visible change for
    /// a pixel-art texture authored with NEAREST.
    pub texture_filters: bool,
    /// Texture wrap modes (repeat / clamp / mirror) survive. Otherwise every
    /// texture repeats, the default in every container.
    pub texture_wrap: bool,
    /// `KHR_texture_transform` (offset, rotation, scale) survives as such.
    /// Otherwise it is BAKED into the texture coordinates of the primitives
    /// that use it, which renders identically whenever one transform applies
    /// per UV set — the case in practice.
    pub texture_transforms: bool,
}

impl Capabilities {
    /// Everything the scene can express (glTF).
    pub const fn full() -> Capabilities {
        Capabilities {
            hierarchy: true,
            normals: true,
            uv_sets: 8,
            vertex_colors: true,
            materials: MaterialSupport::Pbr,
            textures: true,
            skinning: true,
            morph_targets: true,
            animation: true,
            multiple_meshes: true,
            texture_filters: true,
            texture_wrap: true,
            texture_transforms: true,
        }
    }

    pub const fn none() -> Capabilities {
        Capabilities {
            hierarchy: false,
            normals: false,
            uv_sets: 0,
            vertex_colors: false,
            materials: MaterialSupport::None,
            textures: false,
            skinning: false,
            morph_targets: false,
            animation: false,
            multiple_meshes: false,
            texture_filters: false,
            texture_wrap: false,
            texture_transforms: false,
        }
    }

    /// What survives a trip through both `self` and `other`.
    pub fn intersect(self, other: Capabilities) -> Capabilities {
        Capabilities {
            hierarchy: self.hierarchy && other.hierarchy,
            normals: self.normals && other.normals,
            uv_sets: self.uv_sets.min(other.uv_sets),
            vertex_colors: self.vertex_colors && other.vertex_colors,
            materials: self.materials.min(other.materials),
            textures: self.textures && other.textures,
            skinning: self.skinning && other.skinning,
            morph_targets: self.morph_targets && other.morph_targets,
            animation: self.animation && other.animation,
            multiple_meshes: self.multiple_meshes && other.multiple_meshes,
            texture_filters: self.texture_filters && other.texture_filters,
            texture_wrap: self.texture_wrap && other.texture_wrap,
            texture_transforms: self.texture_transforms && other.texture_transforms,
        }
    }
}

/// What kind of content a [`Loss`] is about: a closed, machine-readable
/// vocabulary for API callers (a response header, a dashboard badge), with
/// [`LossKind::as_str`] spellings that are part of the wire contract and do
/// not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LossKind {
    /// Animation clips (the model is delivered at rest).
    Animation,
    /// Skeletons and skin weights (geometry baked into its current pose).
    Skins,
    /// Morph targets (applied at their default weights).
    MorphTargets,
    /// Per-vertex normals.
    Normals,
    /// Texture coordinate sets.
    Uvs,
    /// Per-vertex colour.
    VertexColors,
    /// Texture bindings: a texture that cannot be referenced or applied.
    Textures,
    /// Images in an encoding the target does not accept (WebP, KTX2).
    ImageFormats,
    /// Material properties the target's material model cannot express.
    Materials,
    /// Transparency.
    Alpha,
    /// Node hierarchy, names and grouping (transforms are baked).
    Hierarchy,
    /// Separate meshes or primitives merged into one.
    Meshes,
    /// Texture filtering and wrap modes.
    TextureSampling,
    /// Texture transforms that could not be baked into coordinates.
    TextureTransforms,
    /// Geometry the target cannot represent (degenerate triangles, shear).
    Geometry,
}

impl LossKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LossKind::Animation => "animation",
            LossKind::Skins => "skins",
            LossKind::MorphTargets => "morph_targets",
            LossKind::Normals => "normals",
            LossKind::Uvs => "uvs",
            LossKind::VertexColors => "vertex_colors",
            LossKind::Textures => "textures",
            LossKind::ImageFormats => "image_formats",
            LossKind::Materials => "materials",
            LossKind::Alpha => "alpha",
            LossKind::Hierarchy => "hierarchy",
            LossKind::Meshes => "meshes",
            LossKind::TextureSampling => "texture_sampling",
            LossKind::TextureTransforms => "texture_transforms",
            LossKind::Geometry => "geometry",
        }
    }
}

impl std::fmt::Display for LossKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One thing a conversion could not carry: a machine-readable `kind`, and
/// for people, `what` was affected and `why`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    pub kind: LossKind,
    pub what: String,
    pub why: String,
}

impl Loss {
    pub fn new(kind: LossKind, what: impl Into<String>, why: impl Into<String>) -> Loss {
        Loss { kind, what: what.into(), why: why.into() }
    }
}

impl std::fmt::Display for Loss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.what, self.why)
    }
}

/// Everything in `scene` that a container with `caps` cannot hold, named.
///
/// This is computed from the scene's actual content, not from the format pair:
/// converting a static, untextured mesh to STL loses nothing worth telling
/// anyone about, and saying "animation dropped" for a file that had none would
/// train callers to ignore the report.
pub fn losses(scene: &Scene, caps: &Capabilities, target: &str) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push =
        |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("{target} {why}")));

    let prims: Vec<&Primitive> = scene.meshes.iter().flat_map(|m| &m.primitives).collect();
    if !caps.animation && !scene.animations.is_empty() {
        let names: Vec<String> = scene
            .animations
            .iter()
            .enumerate()
            .map(|(i, a)| a.name.clone().unwrap_or_else(|| format!("#{i}")))
            .collect();
        push(
            LossKind::Animation,
            format!("{} animation(s) [{}]", names.len(), names.join(", ")),
            "cannot hold animation; the model is posed as it stands with no clip playing",
        );
    }
    if !caps.skinning && !scene.skins.is_empty() {
        push(
            LossKind::Skins,
            format!("{} skin(s)", scene.skins.len()),
            "cannot hold a skeleton; skinned geometry is baked into its current pose",
        );
    }
    let morphs = scene.meshes.iter().filter(|m| m.target_count() > 0).count();
    if !caps.morph_targets && morphs > 0 {
        push(
            LossKind::MorphTargets,
            format!("morph targets on {morphs} mesh(es)"),
            "cannot hold morph targets; they are applied at their default weights",
        );
    }
    if !caps.normals && prims.iter().any(|p| p.normals.is_some()) {
        push(LossKind::Normals, "vertex normals".into(), "stores no per-vertex normals; readers recompute them");
    }
    let max_uv = prims.iter().map(|p| p.uvs.len()).max().unwrap_or(0);
    if max_uv > caps.uv_sets {
        push(
            LossKind::Uvs,
            format!("{} of {max_uv} uv set(s)", max_uv - caps.uv_sets),
            "cannot hold that many uv sets",
        );
    }
    if !caps.vertex_colors && prims.iter().any(|p| p.colors.is_some()) {
        push(LossKind::VertexColors, "vertex colours".into(), "cannot hold per-vertex colour");
    }
    let used_textures = scene.materials.iter().flat_map(|m| m.texture_refs()).count();
    if !caps.textures && used_textures > 0 {
        push(LossKind::Textures, format!("{used_textures} texture binding(s)"), "cannot embed or reference textures");
    }
    if caps.textures {
        let unportable: Vec<String> = scene
            .images
            .iter()
            .enumerate()
            .filter(|(_, im)| !im.format.is_universal())
            .map(|(i, im)| format!("image {i} ({})", im.format.mime()))
            .collect();
        if !unportable.is_empty() && !target.starts_with("gl") {
            push(LossKind::ImageFormats, unportable.join(", "), "accepts only PNG and JPEG textures; these are dropped");
        }
    }
    match caps.materials {
        MaterialSupport::None if !scene.materials.is_empty() => {
            push(LossKind::Materials, format!("{} material(s)", scene.materials.len()), "has no materials");
        }
        MaterialSupport::VertexColor if !scene.materials.is_empty() => {
            push(
                LossKind::Materials,
                "materials".into(),
                "has no materials; each material's base colour is baked into its vertices' colours",
            );
        }
        MaterialSupport::ColorOnly if !scene.materials.is_empty() => {
            push(LossKind::Materials, "material properties other than colour".into(), "stores colour only");
        }
        MaterialSupport::Phong
            if scene.materials.iter().any(|m| {
                m.metallic_roughness_texture.is_some() || m.occlusion_texture.is_some() || m.metallic > 0.0
            }) =>
        {
            push(
                LossKind::Materials,
                "metallic/roughness/occlusion".into(),
                "has a diffuse-specular material model; PBR terms are approximated",
            );
        }
        _ => {}
    }
    if !caps.hierarchy && scene.nodes.len() > 1 {
        push(
            LossKind::Hierarchy,
            format!("{} node(s) of hierarchy", scene.nodes.len()),
            "is flat; transforms are baked into vertices",
        );
    }
    if !caps.multiple_meshes && prims.len() > 1 {
        push(LossKind::Meshes, format!("{} separate primitive(s)", prims.len()), "holds one mesh; they are merged");
    }
    if caps.textures {
        // Sampling state only matters for textures that survive at all.
        let custom_filters = scene.textures.iter().any(|t| t.sampler.mag_filter.is_some() || t.sampler.min_filter.is_some());
        let custom_wrap = scene.textures.iter().any(|t| t.sampler.wrap_s != Wrap::Repeat || t.sampler.wrap_t != Wrap::Repeat);
        if (!caps.texture_filters && custom_filters) || (!caps.texture_wrap && custom_wrap) {
            push(
                LossKind::TextureSampling,
                "texture filtering or wrap modes".into(),
                "cannot hold them; readers filter and repeat textures their default way",
            );
        }
        if !caps.texture_transforms {
            let conflicts = conflicting_texture_transforms(scene);
            if conflicts > 0 {
                push(
                    LossKind::TextureTransforms,
                    format!("texture transforms on {conflicts} material(s)"),
                    "cannot hold texture transforms, and these apply different ones to one uv set, so only the first can be baked into the coordinates",
                );
            }
        }
    }
    out
}

/// Materials that apply two different texture transforms to one UV set: the
/// case `pose::bake_texture_transforms` cannot make lossless.
fn conflicting_texture_transforms(scene: &Scene) -> usize {
    scene
        .materials
        .iter()
        .filter(|m| {
            let refs: Vec<&TextureRef> = m.texture_refs().map(|(_, r)| r).collect();
            refs.iter().enumerate().any(|(i, a)| refs[i + 1..].iter().any(|b| b.uv_set == a.uv_set && b.transform != a.transform))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn tri_prim() -> Primitive {
        Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Default::default()
        }
    }

    fn one_node_scene() -> Scene {
        let mut s = Scene {
            meshes: vec![SceneMesh { primitives: vec![tri_prim()], ..Default::default() }],
            ..Default::default()
        };
        s.add_root(Node { mesh: Some(0), ..Default::default() });
        s
    }

    #[test]
    fn a_minimal_scene_validates() {
        one_node_scene().validate().unwrap();
    }

    #[test]
    fn a_node_with_two_parents_is_refused() {
        let mut s = one_node_scene();
        s.nodes.push(Node { children: vec![0], ..Default::default() });
        s.nodes.push(Node { children: vec![0], ..Default::default() });
        s.roots = vec![1, 2];
        assert!(matches!(s.validate(), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn a_cycle_is_refused() {
        let mut s = one_node_scene();
        s.nodes.push(Node { children: vec![2], ..Default::default() });
        s.nodes.push(Node { children: vec![1], ..Default::default() });
        let e = s.validate().unwrap_err();
        assert!(matches!(e, MeshError::Malformed(_)), "{e:?}");
    }

    #[test]
    fn primitives_of_one_mesh_must_agree_on_target_count() {
        let mut s = one_node_scene();
        let mut p = tri_prim();
        p.targets.push(MorphTarget { positions: vec![[0.0; 3]; 3], normals: None });
        s.meshes[0].primitives.push(p);
        assert!(matches!(s.validate(), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn skin_influences_must_be_a_multiple_of_four() {
        let mut s = one_node_scene();
        s.meshes[0].primitives[0].skin =
            Some(SkinWeights { influences: 3, joints: vec![0; 9], weights: vec![1.0; 9] });
        assert!(matches!(s.validate(), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn animation_values_must_match_the_key_count() {
        let mut s = one_node_scene();
        s.animations.push(Animation {
            name: None,
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0, 1.0],
                values: vec![0.0; 5],
            }],
        });
        assert!(matches!(s.validate(), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn key_times_must_increase() {
        let mut s = one_node_scene();
        s.animations.push(Animation {
            name: None,
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Step,
                times: vec![1.0, 1.0],
                values: vec![0.0; 6],
            }],
        });
        assert!(matches!(s.validate(), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn losses_name_only_what_the_scene_actually_has() {
        // Nothing to lose: a bare triangle into the poorest container.
        assert!(losses(&one_node_scene(), &Capabilities::none(), "stl").is_empty());

        let mut s = one_node_scene();
        s.materials.push(Material::default());
        s.meshes[0].primitives[0].material = Some(0);
        s.animations.push(Animation {
            name: Some("Wave".into()),
            channels: vec![Channel {
                node: 0,
                path: AnimPath::Translation,
                interpolation: Interpolation::Linear,
                times: vec![0.0],
                values: vec![0.0; 3],
            }],
        });
        let l = losses(&s, &Capabilities::none(), "stl");
        let text: Vec<String> = l.iter().map(|l| l.to_string()).collect();
        assert!(text.iter().any(|t| t.contains("Wave")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("material")), "{text:?}");
    }

    #[test]
    fn image_sniffing_trusts_magic_not_labels() {
        assert_eq!(ImageFormat::sniff(b"\x89PNG\r\n\x1a\nrest"), Some(ImageFormat::Png));
        assert_eq!(ImageFormat::sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(ImageFormat::sniff(b"GIF89a"), None);
    }

    #[test]
    fn uv_transform_matches_the_khr_texture_transform_definition() {
        // offset then rotation then scale, as a 3x3 applied to (u, v, 1):
        // rotating (1, 0) by +90° under glTF's matrix gives (0, -1).
        let t = UvTransform { offset: [0.0, 0.0], rotation: std::f32::consts::FRAC_PI_2, scale: [1.0, 1.0] };
        let r = t.apply([1.0, 0.0]);
        assert!(r[0].abs() < 1e-6 && (r[1] + 1.0).abs() < 1e-6, "{r:?}");
        let t = UvTransform { offset: [0.5, 0.25], rotation: 0.0, scale: [2.0, 3.0] };
        assert_eq!(t.apply([1.0, 1.0]), [2.5, 3.25]);
    }
}
