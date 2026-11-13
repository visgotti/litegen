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
//! - lights shine along their node's local −Z, and cameras look along it with
//!   +Y up; light intensity is in candela (point, spot) or lux (directional);
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
/// Morph-target floats a reader may build (targets × vertices × 3, and as
/// much again for normal offsets): the decoded-value ceiling `glb.rs` applies.
pub const MAX_MORPH_VALUES: usize = 48_000_000;
/// Joint entries across every skin a reader builds. A skin copies its
/// skeleton's joints and inverse binds, and USD builds one per distinct
/// `geomBindTransform`: 2,000 meshes on a 2,000-joint skeleton were 4M.
pub const MAX_SKIN_ENTRIES: usize = 1_000_000;

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
    /// The lights nodes place ([`Node::light`]).
    pub lights: Vec<Light>,
    /// The cameras nodes place ([`Node::camera`]).
    pub cameras: Vec<Camera>,
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
    /// `litegen_…` names a root litegen added itself (see
    /// [`Node::is_litegen_root`]).
    pub name: Option<String>,
    pub transform: Transform,
    pub children: Vec<usize>,
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    /// Morph weights for this instance, overriding the mesh's defaults.
    pub weights: Option<Vec<f32>>,
    /// The light this node places ([`Scene::lights`]), shining along the
    /// node's local −Z.
    pub light: Option<usize>,
    /// The camera this node places ([`Scene::cameras`]), looking along the
    /// node's local −Z with +Y up.
    pub camera: Option<usize>,
}

/// The prefix of the roots litegen adds itself (`normalize`'s orientation,
/// placement and the converter's units and axis).
pub const LITEGEN_ROOT_PREFIX: &str = "litegen_";

impl Node {
    /// A root litegen added to turn, place or scale the whole model: no mesh,
    /// no skin, named `litegen_…`. Flattening one loses nothing of the input.
    pub fn is_litegen_root(&self) -> bool {
        self.mesh.is_none() && self.skin.is_none() && self.name.as_deref().is_some_and(|n| n.starts_with(LITEGEN_ROOT_PREFIX))
    }
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

/// One draw: triangles, line segments or points ([`Topology`]) with one
/// material.
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
    /// A list of what `topology` draws: a triangle list, a line list (each
    /// pair of indices one segment) or a point list. Strips, loops and fans
    /// are expanded on read, and points without indices get `0..n`.
    pub indices: Vec<u32>,
    pub material: Option<usize>,
    pub targets: Vec<MorphTarget>,
    /// What `indices` draws: glTF's `primitive.mode` 4 (TRIANGLES), 1 (LINES)
    /// or 0 (POINTS). Triangles unless a reader says otherwise.
    pub topology: Topology,
}

/// What a [`Primitive`]'s indices draw: glTF's `primitive.mode`, as lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Topology {
    /// Mode 4: every three indices one triangle.
    #[default]
    Triangles,
    /// Mode 1: every two indices one segment.
    Lines,
    /// Mode 0: every index one point.
    Points,
}

impl Topology {
    /// How many indices draw one element: three for a triangle, two for a
    /// segment, one for a point.
    pub const fn indices_per_element(self) -> usize {
        match self {
            Topology::Triangles => 3,
            Topology::Lines => 2,
            Topology::Points => 1,
        }
    }
}

impl Primitive {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    /// The triangles it draws: none for lines or points.
    pub fn triangle_count(&self) -> usize {
        if self.topology == Topology::Triangles { self.indices.len() / 3 } else { 0 }
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
/// surface looks like and that other containers can express, and glTF's PBR
/// layer extensions over it ([`MaterialLayer`]), each `None` when absent. A
/// layer's fields are its extension's, with its defaults and units, named
/// without the layer's prefix or `Factor` (`clearcoatRoughnessFactor` is
/// `Clearcoat::roughness`); each field's doc names its glTF property.
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
    /// `KHR_materials_clearcoat`.
    pub clearcoat: Option<Clearcoat>,
    /// `KHR_materials_transmission`.
    pub transmission: Option<Transmission>,
    /// `KHR_materials_volume`.
    pub volume: Option<Volume>,
    /// `KHR_materials_ior`'s index of refraction; 1.5 when absent.
    pub ior: Option<f32>,
    /// `KHR_materials_specular`.
    pub specular: Option<Specular>,
    /// `KHR_materials_sheen`.
    pub sheen: Option<Sheen>,
    /// `KHR_materials_iridescence`.
    pub iridescence: Option<Iridescence>,
    /// `KHR_materials_anisotropy`.
    pub anisotropy: Option<Anisotropy>,
    /// `KHR_materials_dispersion`: 20 / the Abbe number; 0 when absent.
    pub dispersion: Option<f32>,
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
            clearcoat: None,
            transmission: None,
            volume: None,
            ior: None,
            specular: None,
            sheen: None,
            iridescence: None,
            anisotropy: None,
            dispersion: None,
        }
    }
}

// Each layer's `Default` is its extension's own defaults, so a layer read
// from a file that omits a field means what the extension says it means.

/// `KHR_materials_clearcoat`: a clear coat over the base layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Clearcoat {
    /// `clearcoatFactor`, 0 to 1.
    pub factor: f32,
    /// `clearcoatTexture`: the factor in R.
    pub texture: Option<TextureRef>,
    /// `clearcoatRoughnessFactor`, 0 to 1.
    pub roughness: f32,
    /// `clearcoatRoughnessTexture`: the roughness in G.
    pub roughness_texture: Option<TextureRef>,
    /// `clearcoatNormalTexture`: the coat's own normal map.
    pub normal_texture: Option<TextureRef>,
    /// The clearcoat normal texture's `scale`.
    pub normal_scale: f32,
}

impl Default for Clearcoat {
    fn default() -> Self {
        Clearcoat { factor: 0.0, texture: None, roughness: 0.0, roughness_texture: None, normal_texture: None, normal_scale: 1.0 }
    }
}

/// `KHR_materials_transmission`: light passing through the surface (glass).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transmission {
    /// `transmissionFactor`, 0 to 1; 0 by default.
    pub factor: f32,
    /// `transmissionTexture`: the factor in R.
    pub texture: Option<TextureRef>,
}

/// `KHR_materials_volume`: the medium under a transmissive surface.
#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    /// `thicknessFactor`, in the mesh's own space; 0 is thin-walled.
    pub thickness: f32,
    /// `thicknessTexture`: the thickness in G.
    pub thickness_texture: Option<TextureRef>,
    /// `attenuationDistance`, in world space (metres): how far light travels
    /// before it is absorbed to `attenuation_color`. `f32::INFINITY`, the
    /// extension's default, is no absorption.
    pub attenuation_distance: f32,
    /// `attenuationColor`, linear RGB.
    pub attenuation_color: [f32; 3],
}

impl Default for Volume {
    fn default() -> Self {
        Volume { thickness: 0.0, thickness_texture: None, attenuation_distance: f32::INFINITY, attenuation_color: [1.0; 3] }
    }
}

/// `KHR_materials_specular`: the strength and colour of a dielectric's
/// specular reflection.
#[derive(Debug, Clone, PartialEq)]
pub struct Specular {
    /// `specularFactor`, 0 to 1.
    pub factor: f32,
    /// `specularTexture`: the strength in A.
    pub texture: Option<TextureRef>,
    /// `specularColorFactor`, linear RGB.
    pub color: [f32; 3],
    /// `specularColorTexture`: RGB, sRGB-encoded.
    pub color_texture: Option<TextureRef>,
}

impl Default for Specular {
    fn default() -> Self {
        Specular { factor: 1.0, texture: None, color: [1.0; 3], color_texture: None }
    }
}

/// `KHR_materials_sheen`: the back-scattering of cloth fibres.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheen {
    /// `sheenColorFactor`, linear RGB; black (no sheen) by default.
    pub color: [f32; 3],
    /// `sheenColorTexture`: RGB, sRGB-encoded.
    pub color_texture: Option<TextureRef>,
    /// `sheenRoughnessFactor`, 0 to 1; 0 by default.
    pub roughness: f32,
    /// `sheenRoughnessTexture`: the roughness in A.
    pub roughness_texture: Option<TextureRef>,
}

/// `KHR_materials_iridescence`: a thin film over the surface (soap bubbles,
/// oil slicks).
#[derive(Debug, Clone, PartialEq)]
pub struct Iridescence {
    /// `iridescenceFactor`, 0 to 1.
    pub factor: f32,
    /// `iridescenceTexture`: the factor in R.
    pub texture: Option<TextureRef>,
    /// `iridescenceIor`: the film's index of refraction.
    pub ior: f32,
    /// `iridescenceThicknessMinimum`, in nanometres.
    pub thickness_min: f32,
    /// `iridescenceThicknessMaximum`, in nanometres.
    pub thickness_max: f32,
    /// `iridescenceThicknessTexture`: G blends from the minimum to the
    /// maximum thickness.
    pub thickness_texture: Option<TextureRef>,
}

impl Default for Iridescence {
    fn default() -> Self {
        Iridescence { factor: 0.0, texture: None, ior: 1.3, thickness_min: 100.0, thickness_max: 400.0, thickness_texture: None }
    }
}

/// `KHR_materials_anisotropy`: highlights stretched along a direction
/// (brushed metal).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Anisotropy {
    /// `anisotropyStrength`, 0 to 1; 0 by default.
    pub strength: f32,
    /// `anisotropyRotation`: radians counter-clockwise from the tangent; 0 by
    /// default.
    pub rotation: f32,
    /// `anisotropyTexture`: the direction in RG, the strength in B.
    pub texture: Option<TextureRef>,
}

impl Material {
    /// The specular colour, linear, that a Phong material drawing this one
    /// should have: glTF's F0 — 0.04 for a dielectric, the base colour for a
    /// metal (glTF 2.0 Appendix B, `#metallic-roughness-material`) — faded by
    /// `(1 − roughness)²`, so a fully rough surface has no highlight. Phong
    /// shininess 0, what full roughness maps to, otherwise lights every point
    /// as highlight, and three.js draws that as a uniform wash.
    pub fn phong_specular(&self) -> [f32; 3] {
        let m = self.metallic.clamp(0.0, 1.0);
        let fade = (1.0 - self.roughness.clamp(0.0, 1.0)).powi(2);
        [0, 1, 2].map(|i| (0.04 * (1.0 - m) + self.base_color[i].clamp(0.0, 1.0) * m) * fade)
    }

    /// The alpha its colour is drawn with: glTF ignores it when OPAQUE, and
    /// MASK draws a constant colour wholly or not at all (glTF 2.0 §3.9.4).
    /// What a container with only a colour's alpha to say it with writes.
    pub fn drawn_alpha(&self) -> f32 {
        match self.alpha_mode {
            AlphaMode::Opaque => 1.0,
            AlphaMode::Mask if self.base_color[3] >= self.alpha_cutoff => 1.0,
            AlphaMode::Mask => 0.0,
            AlphaMode::Blend => self.base_color[3],
        }
    }

    /// [`Material::texture_slots_mut`], shared.
    fn texture_slots(&self) -> impl Iterator<Item = (TextureSlot, &Option<TextureRef>)> {
        let clearcoat = self.clearcoat.as_ref().map(|c| {
            [
                (TextureSlot::Clearcoat, &c.texture),
                (TextureSlot::ClearcoatRoughness, &c.roughness_texture),
                (TextureSlot::ClearcoatNormal, &c.normal_texture),
            ]
        });
        let transmission = self.transmission.as_ref().map(|t| [(TextureSlot::Transmission, &t.texture)]);
        let volume = self.volume.as_ref().map(|v| [(TextureSlot::VolumeThickness, &v.thickness_texture)]);
        let specular = self.specular.as_ref().map(|s| [(TextureSlot::Specular, &s.texture), (TextureSlot::SpecularColor, &s.color_texture)]);
        let sheen = self.sheen.as_ref().map(|s| [(TextureSlot::SheenColor, &s.color_texture), (TextureSlot::SheenRoughness, &s.roughness_texture)]);
        let iridescence = self
            .iridescence
            .as_ref()
            .map(|i| [(TextureSlot::Iridescence, &i.texture), (TextureSlot::IridescenceThickness, &i.thickness_texture)]);
        let anisotropy = self.anisotropy.as_ref().map(|a| [(TextureSlot::Anisotropy, &a.texture)]);
        [
            (TextureSlot::BaseColor, &self.base_color_texture),
            (TextureSlot::MetallicRoughness, &self.metallic_roughness_texture),
            (TextureSlot::Normal, &self.normal_texture),
            (TextureSlot::Occlusion, &self.occlusion_texture),
            (TextureSlot::Emissive, &self.emissive_texture),
        ]
        .into_iter()
        .chain(clearcoat.into_iter().flatten())
        .chain(transmission.into_iter().flatten())
        .chain(volume.into_iter().flatten())
        .chain(specular.into_iter().flatten())
        .chain(sheen.into_iter().flatten())
        .chain(iridescence.into_iter().flatten())
        .chain(anisotropy.into_iter().flatten())
    }

    /// Every texture slot the material has, bound or not, in [`TextureSlot`]
    /// order: the five base slots, then the slots of each layer it has. For a
    /// pass that drops or replaces bindings; [`Material::texture_refs_mut`]
    /// gives the bound ones.
    pub fn texture_slots_mut(&mut self) -> impl Iterator<Item = (TextureSlot, &mut Option<TextureRef>)> {
        let clearcoat = self.clearcoat.as_mut().map(|c| {
            [
                (TextureSlot::Clearcoat, &mut c.texture),
                (TextureSlot::ClearcoatRoughness, &mut c.roughness_texture),
                (TextureSlot::ClearcoatNormal, &mut c.normal_texture),
            ]
        });
        let transmission = self.transmission.as_mut().map(|t| [(TextureSlot::Transmission, &mut t.texture)]);
        let volume = self.volume.as_mut().map(|v| [(TextureSlot::VolumeThickness, &mut v.thickness_texture)]);
        let specular =
            self.specular.as_mut().map(|s| [(TextureSlot::Specular, &mut s.texture), (TextureSlot::SpecularColor, &mut s.color_texture)]);
        let sheen = self
            .sheen
            .as_mut()
            .map(|s| [(TextureSlot::SheenColor, &mut s.color_texture), (TextureSlot::SheenRoughness, &mut s.roughness_texture)]);
        let iridescence = self
            .iridescence
            .as_mut()
            .map(|i| [(TextureSlot::Iridescence, &mut i.texture), (TextureSlot::IridescenceThickness, &mut i.thickness_texture)]);
        let anisotropy = self.anisotropy.as_mut().map(|a| [(TextureSlot::Anisotropy, &mut a.texture)]);
        [
            (TextureSlot::BaseColor, &mut self.base_color_texture),
            (TextureSlot::MetallicRoughness, &mut self.metallic_roughness_texture),
            (TextureSlot::Normal, &mut self.normal_texture),
            (TextureSlot::Occlusion, &mut self.occlusion_texture),
            (TextureSlot::Emissive, &mut self.emissive_texture),
        ]
        .into_iter()
        .chain(clearcoat.into_iter().flatten())
        .chain(transmission.into_iter().flatten())
        .chain(volume.into_iter().flatten())
        .chain(specular.into_iter().flatten())
        .chain(sheen.into_iter().flatten())
        .chain(iridescence.into_iter().flatten())
        .chain(anisotropy.into_iter().flatten())
    }

    /// The bound texture slots, base and layers, in [`TextureSlot`] order.
    pub fn texture_refs(&self) -> impl Iterator<Item = (TextureSlot, &TextureRef)> {
        self.texture_slots().filter_map(|(slot, r)| r.as_ref().map(|r| (slot, r)))
    }

    /// [`Material::texture_refs`], mutably, in the same slot order.
    pub fn texture_refs_mut(&mut self) -> impl Iterator<Item = (TextureSlot, &mut TextureRef)> {
        self.texture_slots_mut().filter_map(|(slot, r)| r.as_mut().map(|r| (slot, r)))
    }

    pub fn has_layer(&self, layer: MaterialLayer) -> bool {
        match layer {
            MaterialLayer::Clearcoat => self.clearcoat.is_some(),
            MaterialLayer::Transmission => self.transmission.is_some(),
            MaterialLayer::Volume => self.volume.is_some(),
            MaterialLayer::Ior => self.ior.is_some(),
            MaterialLayer::Specular => self.specular.is_some(),
            MaterialLayer::Sheen => self.sheen.is_some(),
            MaterialLayer::Iridescence => self.iridescence.is_some(),
            MaterialLayer::Anisotropy => self.anisotropy.is_some(),
            MaterialLayer::Dispersion => self.dispersion.is_some(),
        }
    }

    /// Remove `layer`, its factors and its textures; the base layer stays.
    pub fn drop_layer(&mut self, layer: MaterialLayer) {
        match layer {
            MaterialLayer::Clearcoat => self.clearcoat = None,
            MaterialLayer::Transmission => self.transmission = None,
            MaterialLayer::Volume => self.volume = None,
            MaterialLayer::Ior => self.ior = None,
            MaterialLayer::Specular => self.specular = None,
            MaterialLayer::Sheen => self.sheen = None,
            MaterialLayer::Iridescence => self.iridescence = None,
            MaterialLayer::Anisotropy => self.anisotropy = None,
            MaterialLayer::Dispersion => self.dispersion = None,
        }
    }
}

/// glTF's PBR layer extensions, the optional layers of a [`Material`], in
/// its field order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialLayer {
    Clearcoat,
    Transmission,
    Volume,
    Ior,
    Specular,
    Sheen,
    Iridescence,
    Anisotropy,
    Dispersion,
}

impl MaterialLayer {
    pub const ALL: [MaterialLayer; 9] = [
        MaterialLayer::Clearcoat,
        MaterialLayer::Transmission,
        MaterialLayer::Volume,
        MaterialLayer::Ior,
        MaterialLayer::Specular,
        MaterialLayer::Sheen,
        MaterialLayer::Iridescence,
        MaterialLayer::Anisotropy,
        MaterialLayer::Dispersion,
    ];

    /// The extension's name without `KHR_materials_`, as losses name it.
    pub fn as_str(self) -> &'static str {
        match self {
            MaterialLayer::Clearcoat => "clearcoat",
            MaterialLayer::Transmission => "transmission",
            MaterialLayer::Volume => "volume",
            MaterialLayer::Ior => "ior",
            MaterialLayer::Specular => "specular",
            MaterialLayer::Sheen => "sheen",
            MaterialLayer::Iridescence => "iridescence",
            MaterialLayer::Anisotropy => "anisotropy",
            MaterialLayer::Dispersion => "dispersion",
        }
    }

    /// The layer's texture slots, in [`TextureSlot`] order; none for `ior`
    /// and `dispersion`, which are numbers alone.
    pub fn texture_slots(self) -> &'static [TextureSlot] {
        match self {
            MaterialLayer::Clearcoat => &[TextureSlot::Clearcoat, TextureSlot::ClearcoatRoughness, TextureSlot::ClearcoatNormal],
            MaterialLayer::Transmission => &[TextureSlot::Transmission],
            MaterialLayer::Volume => &[TextureSlot::VolumeThickness],
            MaterialLayer::Ior | MaterialLayer::Dispersion => &[],
            MaterialLayer::Specular => &[TextureSlot::Specular, TextureSlot::SpecularColor],
            MaterialLayer::Sheen => &[TextureSlot::SheenColor, TextureSlot::SheenRoughness],
            MaterialLayer::Iridescence => &[TextureSlot::Iridescence, TextureSlot::IridescenceThickness],
            MaterialLayer::Anisotropy => &[TextureSlot::Anisotropy],
        }
    }
}

/// A material's texture slots: the five base ones, then each layer's
/// ([`MaterialLayer::texture_slots`]), in the order [`Material::texture_refs`]
/// yields them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureSlot {
    BaseColor,
    MetallicRoughness,
    Normal,
    Occlusion,
    Emissive,
    Clearcoat,
    ClearcoatRoughness,
    ClearcoatNormal,
    Transmission,
    VolumeThickness,
    Specular,
    SpecularColor,
    SheenColor,
    SheenRoughness,
    Iridescence,
    IridescenceThickness,
    Anisotropy,
}

impl TextureSlot {
    /// glTF's name for the slot's texture without `Texture`: `baseColor`,
    /// `clearcoatRoughness`, and `thickness` for `KHR_materials_volume`'s.
    pub fn as_str(self) -> &'static str {
        match self {
            TextureSlot::BaseColor => "baseColor",
            TextureSlot::MetallicRoughness => "metallicRoughness",
            TextureSlot::Normal => "normal",
            TextureSlot::Occlusion => "occlusion",
            TextureSlot::Emissive => "emissive",
            TextureSlot::Clearcoat => "clearcoat",
            TextureSlot::ClearcoatRoughness => "clearcoatRoughness",
            TextureSlot::ClearcoatNormal => "clearcoatNormal",
            TextureSlot::Transmission => "transmission",
            TextureSlot::VolumeThickness => "thickness",
            TextureSlot::Specular => "specular",
            TextureSlot::SpecularColor => "specularColor",
            TextureSlot::SheenColor => "sheenColor",
            TextureSlot::SheenRoughness => "sheenRoughness",
            TextureSlot::Iridescence => "iridescence",
            TextureSlot::IridescenceThickness => "iridescenceThickness",
            TextureSlot::Anisotropy => "anisotropy",
        }
    }

    /// Colour textures are sRGB-encoded; data textures are linear. The
    /// distinction matters to every writer that has to label it (USD's
    /// `sourceColorSpace`, 3MF's colour handling).
    pub fn is_color(self) -> bool {
        matches!(self, TextureSlot::BaseColor | TextureSlot::Emissive | TextureSlot::SpecularColor | TextureSlot::SheenColor)
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

impl Sampler {
    /// Whether it filters as a reader does when a file says nothing: linear
    /// magnification, trilinear minification (three.js r170
    /// GLTFLoader.js:3230-3231's fallbacks, and what "unspecified" means in
    /// practice). Only a sampler that says otherwise has anything for a
    /// container without filter settings to lose.
    pub fn filters_by_default(&self) -> bool {
        matches!(self.mag_filter, None | Some(Filter::Linear))
            && match self.min_filter {
                None => true,
                Some(Filter::Linear) => self.mipmaps && self.mipmap_filter == Filter::Linear,
                Some(_) => false,
            }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub name: Option<String>,
    pub image: usize,
    pub sampler: Sampler,
}

/// An encoded image's format. A scene carries WebP and KTX2 (Basis Universal)
/// images as the bytes they came in; `normalize::transcode_textures` decodes
/// both (KTX2 through `ktx2::decode_rgba8`) and re-encodes them as PNG, which
/// every container and consumer reads. glTF alone keeps a KTX2 image as it is
/// (`KHR_texture_basisu`), when it is written from a scene nobody transcoded.
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
    /// else, so a WebP or KTX2 texture cannot be carried into them as it is:
    /// `normalize::transcode_textures` makes PNG of both first, and a writer
    /// that is handed one anyway reports it lost.
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

// ─── lights and cameras ─────────────────────────────────────────────────────

/// A punctual light, glTF's `KHR_lights_punctual`, placed by a node
/// ([`Node::light`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Light {
    pub name: Option<String>,
    pub kind: LightKind,
    /// Linear RGB, each component 0 to 1.
    pub color: [f32; 3],
    /// Candela for a point or spot light, lux for a directional one.
    pub intensity: f32,
    /// The distance from a point or spot light, in metres, at which its
    /// intensity may be taken to have reached zero; `None` is infinite.
    pub range: Option<f32>,
}

/// How a [`Light`] shines. A directional or spot light shines along its
/// node's local −Z.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LightKind {
    /// Parallel rays, as from the sun: where the node stands does not matter.
    Directional,
    /// Every way from the node's origin.
    Point,
    /// A cone from the node's origin: full intensity inside `inner_cone`,
    /// falling to none at `outer_cone`.
    Spot {
        /// A half-angle from the axis, in radians.
        inner_cone: f32,
        /// A half-angle from the axis, in radians.
        outer_cone: f32,
    },
}

/// Blender's glTF importer's lumens per watt, `PBR_WATTS_TO_LUMENS` (the
/// io_scene_gltf2 add-on bundled with Blender 5.2, v5.2.39,
/// `blender/com/conversion.py` L10).
pub const BLENDER_LUMENS_PER_WATT: f64 = 683.0;

/// The `energy` Blender's own glTF importer gives a light of `intensity`
/// (candela for a point or spot light, lux for a directional one), in its
/// default `SPEC` lighting mode: a sun gets lux / 683 W/m², a point or spot
/// light candela × 4π / 683 W (`blender/imp/light.py` L58-L66 and L69-L77,
/// v5.2.39, from the bundled 5.2.0; the 4π is the sphere's solid angle).
/// Blender's spot `energy` is the power of the point light that has the same
/// intensity, so a spot's cone leaves it unchanged. [`from_blender_energy`]
/// is the inverse. The unit options of the FBX and USD codecs (`LightUnits`)
/// write and read this.
pub fn blender_energy(kind: &LightKind, intensity: f64) -> f64 {
    match kind {
        LightKind::Directional => intensity / BLENDER_LUMENS_PER_WATT,
        LightKind::Point | LightKind::Spot { .. } => intensity * 4.0 * std::f64::consts::PI / BLENDER_LUMENS_PER_WATT,
    }
}

/// The candela or lux whose Blender glTF import gives `energy`: the inverse
/// of [`blender_energy`] for the same kind of light.
pub fn from_blender_energy(kind: &LightKind, energy: f64) -> f64 {
    match kind {
        LightKind::Directional => energy * BLENDER_LUMENS_PER_WATT,
        LightKind::Point | LightKind::Spot { .. } => energy * BLENDER_LUMENS_PER_WATT / (4.0 * std::f64::consts::PI),
    }
}

/// A camera, glTF's `camera`, placed by a node ([`Node::camera`]): it looks
/// along the node's local −Z, with +Y up.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub name: Option<String>,
    pub projection: Projection,
}

/// How a [`Camera`] projects. Distances are in metres along the view axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    Perspective {
        /// The vertical field of view, in radians.
        yfov: f32,
        /// Width over height; `None` takes the viewport's.
        aspect: Option<f32>,
        /// The distance to the near clipping plane.
        znear: f32,
        /// The distance to the far clipping plane; `None` is infinite.
        zfar: Option<f32>,
    },
    Orthographic {
        /// Half the width of the view, in metres.
        xmag: f32,
        /// Half the height of the view, in metres.
        ymag: f32,
        /// The distance to the near clipping plane.
        znear: f32,
        /// The distance to the far clipping plane.
        zfar: f32,
    },
}

/// `v` when it is finite, or the reason [`Light::check`] and
/// [`Camera::check`] give for it.
fn finite(what: &str, v: f32) -> Result<f32, String> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(format!("its {what} {v} is not finite"))
    }
}

impl Light {
    /// Whether the light holds glTF's bounds: every value finite, each
    /// colour component 0 to 1, an intensity of 0 or more, a range over 0
    /// when it has one, and a spot's `0 ≤ inner_cone < outer_cone ≤ π/2`
    /// (KHR_lights_punctual's `light.schema.json` and
    /// `light.spot.schema.json`). [`Scene::validate`] refuses a scene with a
    /// light that does not, and a reader drops the light with a warning.
    /// The `Err` names the bound broken and the value, worded to follow
    /// `light "lamp" is dropped: `.
    pub fn check(&self) -> Result<(), String> {
        let c = self.color;
        if !finite3(&c) {
            return Err(format!("its colour {c:?} is not finite"));
        }
        if c.iter().any(|v| !(0.0..=1.0).contains(v)) {
            return Err(format!("its colour {c:?} is outside 0 to 1"));
        }
        if finite("intensity", self.intensity)? < 0.0 {
            return Err(format!("its intensity {} is negative", self.intensity));
        }
        if let Some(r) = self.range {
            if finite("range", r)? <= 0.0 {
                return Err(format!("its range {r} is not positive"));
            }
        }
        if let LightKind::Spot { inner_cone, outer_cone } = self.kind {
            let (inner, outer) = (finite("inner cone", inner_cone)?, finite("outer cone", outer_cone)?);
            if inner < 0.0 {
                return Err(format!("its inner cone {inner} is negative"));
            }
            if inner >= outer {
                return Err(format!("its inner cone {inner} is not below its outer cone {outer}"));
            }
            // f32 π/2 is a hair over π/2; it passes, as refusing it would drop real π/2 spots, and writers clamp it.
            if outer > std::f32::consts::FRAC_PI_2 {
                return Err(format!("its outer cone {outer} is over π/2"));
            }
        }
        Ok(())
    }

    /// Softens a hard-edged spot, one whose inner cone equals its outer one:
    /// full intensity up to the edge and none past it. glTF forbids that
    /// (KHR_lights_punctual's `innerConeAngle` must be less than its
    /// `outerConeAngle`) and [`Light::check`] refuses it, but FBX, Collada
    /// and USD files carry hard edges routinely, so a reader softens one
    /// rather than drop the light: its inner cone moves to
    /// `outer × (1 − 1e-4)`. Returns what it did, worded to follow
    /// `light "lamp" `, or `None` when it did nothing: for any other light,
    /// and for equal cones that are not a positive finite angle, which
    /// `check` then refuses.
    ///
    /// Why 1e-4 of the outer cone: the gap must outlast every codec's
    /// conversion, or the spot comes back hard and is softened again on each
    /// round trip. 1e-4 is about 840 f32 steps or more at any angle (an
    /// f32's step is at most 2⁻²³ ≈ 1.2e-7 of it), and each conversion
    /// rounds by a step or so:
    /// glTF's f32 radians come back exact; FBX's full cones in degrees
    /// (twice the half-angle, as f64) and USD's half-angle in f32 degrees,
    /// with its softness, round once on the way out and once back; Collada
    /// writes its degrees as text in full. And it is too small to see:
    /// 6e-5 rad (0.0034°) on a 0.6 rad cone.
    pub fn soften_hard_edge(&mut self) -> Option<String> {
        let LightKind::Spot { inner_cone, outer_cone } = &mut self.kind else { return None };
        let outer = *outer_cone;
        if *inner_cone != outer || !outer.is_finite() || outer <= 0.0 {
            return None;
        }
        let inner = outer * (1.0 - 1e-4);
        // Deep in the subnormals (below about 7e-42) the product rounds back
        // to `outer`.
        if inner >= outer {
            return None;
        }
        *inner_cone = inner;
        Some(format!("had a hard edge, its inner and outer cones both {outer}; it was softened to an inner cone of {inner}"))
    }
}

impl Camera {
    /// Whether the camera holds glTF's bounds (glTF 2.0's
    /// `camera.perspective.schema.json` and `camera.orthographic.schema.json`):
    /// every value finite; a perspective `yfov` between 0 and π, an aspect
    /// ratio over 0 when it has one, and a `znear` over 0; an orthographic
    /// `xmag` and `ymag` other than 0 and a `znear` of 0 or more; and a `zfar`
    /// beyond `znear` wherever there is one. As for [`Light::check`], the
    /// `Err` names the bound broken and the value, worded to follow
    /// `camera "shot" is dropped: `.
    pub fn check(&self) -> Result<(), String> {
        let beyond = |znear: f32, zfar: f32| -> Result<(), String> {
            if finite("zfar", zfar)? > znear {
                Ok(())
            } else {
                Err(format!("its zfar {zfar} is not beyond its znear {znear}"))
            }
        };
        match self.projection {
            Projection::Perspective { yfov, aspect, znear, zfar } => {
                if !(finite("yfov", yfov)? > 0.0 && yfov < std::f32::consts::PI) {
                    return Err(format!("its yfov {yfov} is not between 0 and π"));
                }
                if let Some(a) = aspect {
                    if finite("aspect ratio", a)? <= 0.0 {
                        return Err(format!("its aspect ratio {a} is not positive"));
                    }
                }
                if finite("znear", znear)? <= 0.0 {
                    return Err(format!("its znear {znear} is not positive"));
                }
                zfar.map_or(Ok(()), |zfar| beyond(znear, zfar))
            }
            Projection::Orthographic { xmag, ymag, znear, zfar } => {
                for (what, mag) in [("xmag", xmag), ("ymag", ymag)] {
                    if finite(what, mag)? == 0.0 {
                        return Err(format!("its {what} is 0"));
                    }
                }
                if finite("znear", znear)? < 0.0 {
                    return Err(format!("its znear {znear} is negative"));
                }
                beyond(znear, zfar)
            }
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

/// The numbers of `layer` that must be finite, none when `m` lacks it. The
/// volume's attenuation distance is not one of them: its default, no
/// absorption at all, is +inf.
fn layer_factors(m: &Material, layer: MaterialLayer) -> Vec<f32> {
    match layer {
        MaterialLayer::Clearcoat => m.clearcoat.iter().flat_map(|c| [c.factor, c.roughness, c.normal_scale]).collect(),
        MaterialLayer::Transmission => m.transmission.iter().map(|t| t.factor).collect(),
        MaterialLayer::Volume => m.volume.iter().flat_map(|v| [v.thickness, v.attenuation_color[0], v.attenuation_color[1], v.attenuation_color[2]]).collect(),
        MaterialLayer::Ior => m.ior.into_iter().collect(),
        MaterialLayer::Specular => m.specular.iter().flat_map(|s| [s.factor, s.color[0], s.color[1], s.color[2]]).collect(),
        MaterialLayer::Sheen => m.sheen.iter().flat_map(|s| [s.color[0], s.color[1], s.color[2], s.roughness]).collect(),
        MaterialLayer::Iridescence => m.iridescence.iter().flat_map(|i| [i.factor, i.ior, i.thickness_min, i.thickness_max]).collect(),
        MaterialLayer::Anisotropy => m.anisotropy.iter().flat_map(|a| [a.strength, a.rotation]).collect(),
        MaterialLayer::Dispersion => m.dispersion.into_iter().collect(),
    }
}

/// KHR_materials_ior's default, `"default": 1.5`: the ior a material that
/// states none has, and the base layer's in glTF's BRDF.
pub const DEFAULT_IOR: f32 = 1.5;

/// Whether KHR_materials_ior's schema holds `ior`: 0, its special value, or
/// 1 and above, of a JSON number, which is finite (`"oneOf": [{ "minimum":
/// 0.0, "maximum": 0.0 }, { "minimum": 1.0 }]`, in
/// https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_materials_ior/schema/material.KHR_materials_ior.schema.json).
/// [`Scene::validate`] refuses a material with any other, and each reader
/// leaves one out through this same check.
pub fn ior_in_schema(ior: f32) -> bool {
    ior.is_finite() && (ior == 0.0 || ior >= 1.0)
}

impl Scene {
    /// The triangles in the scene; lines and points are not counted
    /// ([`Scene::drawable_count`] counts them).
    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.triangle_count()).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.meshes.iter().flat_map(|m| &m.primitives).map(|p| p.vertex_count()).sum()
    }

    /// Everything drawn, by element: each triangle, each line segment and
    /// each point (a primitive's indices over [`Topology::indices_per_element`]).
    /// A scene is empty when this is 0.
    pub fn drawable_count(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|m| &m.primitives)
            .map(|p| p.indices.len() / p.topology.indices_per_element())
            .sum()
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
            if let Some(l) = node.light {
                if l >= self.lights.len() {
                    return Err(malformed(format!("node {i} light {l} is out of range")));
                }
            }
            if let Some(c) = node.camera {
                if c >= self.cameras.len() {
                    return Err(malformed(format!("node {i} camera {c} is out of range")));
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
        // Walk up from every node, stopping where an earlier walk ended well:
        // each node is visited once, where walking every node to its root was
        // quadratic in a deep chain. 1: on this walk; 2: reaches a root.
        let mut state = vec![0u8; n_nodes];
        let mut walk = Vec::new();
        for start in 0..n_nodes {
            walk.clear();
            let mut cur = start;
            loop {
                match state[cur] {
                    2 => break,
                    1 => return Err(malformed(format!("node {start} is part of a cycle"))),
                    _ => {}
                }
                state[cur] = 1;
                walk.push(cur);
                match parent[cur] {
                    Some(p) => cur = p,
                    None => break,
                }
            }
            for &n in &walk {
                state[n] = 2;
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
                let per = p.topology.indices_per_element();
                if !p.indices.len().is_multiple_of(per) {
                    return Err(malformed(format!(
                        "{at} index count {} is not a multiple of {per}",
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
            for layer in MaterialLayer::ALL {
                if !layer_factors(mat, layer).iter().all(|v| v.is_finite()) {
                    return Err(malformed(format!("material {mi} has a non-finite {} factor", layer.as_str())));
                }
            }
            if let Some(d) = mat.volume.as_ref().map(|v| v.attenuation_distance).filter(|d| d.is_nan() || *d == f32::NEG_INFINITY) {
                return Err(malformed(format!("material {mi} has a volume attenuation distance of {d}")));
            }
            if let Some(ior) = mat.ior.filter(|&ior| !ior_in_schema(ior)) {
                return Err(malformed(format!("material {mi} has an ior of {ior}; KHR_materials_ior allows 0, or 1 and above")));
            }
        }
        for (ti, t) in self.textures.iter().enumerate() {
            if t.image >= self.images.len() {
                return Err(malformed(format!("texture {ti} image {} is out of range", t.image)));
            }
        }
        for (li, light) in self.lights.iter().enumerate() {
            light.check().map_err(|why| malformed(format!("light {li}: {why}")))?;
        }
        for (ci, camera) in self.cameras.iter().enumerate() {
            camera.check().map_err(|why| malformed(format!("camera {ci}: {why}")))?;
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
    /// The material layers that survive. Any other goes whole, its factors
    /// and textures, and is reported; the base layer under it stays.
    pub layers: LayerSupport,
    /// Lights survive on their nodes. Otherwise each goes, and a node that
    /// held only a light is left as any empty node is: kept where the
    /// hierarchy survives, baked away where it does not.
    pub lights: bool,
    /// Cameras survive on their nodes. Otherwise they go as `lights` do.
    pub cameras: bool,
    /// Point primitives ([`Topology::Points`]) survive. Otherwise each goes
    /// ([`super::pose::strip_topologies`]), and a mesh left with none goes
    /// with it.
    pub points: bool,
    /// Line primitives ([`Topology::Lines`]) survive. Otherwise they go as
    /// `points` do.
    pub lines: bool,
}

/// Which of the [`MaterialLayer`]s a container keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerSupport {
    pub clearcoat: bool,
    pub transmission: bool,
    pub volume: bool,
    pub ior: bool,
    pub specular: bool,
    pub sheen: bool,
    pub iridescence: bool,
    pub anisotropy: bool,
    pub dispersion: bool,
}

impl LayerSupport {
    pub const fn all() -> LayerSupport {
        LayerSupport {
            clearcoat: true,
            transmission: true,
            volume: true,
            ior: true,
            specular: true,
            sheen: true,
            iridescence: true,
            anisotropy: true,
            dispersion: true,
        }
    }

    pub const fn none() -> LayerSupport {
        LayerSupport {
            clearcoat: false,
            transmission: false,
            volume: false,
            ior: false,
            specular: false,
            sheen: false,
            iridescence: false,
            anisotropy: false,
            dispersion: false,
        }
    }

    pub fn keeps(&self, layer: MaterialLayer) -> bool {
        match layer {
            MaterialLayer::Clearcoat => self.clearcoat,
            MaterialLayer::Transmission => self.transmission,
            MaterialLayer::Volume => self.volume,
            MaterialLayer::Ior => self.ior,
            MaterialLayer::Specular => self.specular,
            MaterialLayer::Sheen => self.sheen,
            MaterialLayer::Iridescence => self.iridescence,
            MaterialLayer::Anisotropy => self.anisotropy,
            MaterialLayer::Dispersion => self.dispersion,
        }
    }

    /// The layers both keep.
    pub fn intersect(self, other: LayerSupport) -> LayerSupport {
        LayerSupport {
            clearcoat: self.clearcoat && other.clearcoat,
            transmission: self.transmission && other.transmission,
            volume: self.volume && other.volume,
            ior: self.ior && other.ior,
            specular: self.specular && other.specular,
            sheen: self.sheen && other.sheen,
            iridescence: self.iridescence && other.iridescence,
            anisotropy: self.anisotropy && other.anisotropy,
            dispersion: self.dispersion && other.dispersion,
        }
    }
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
            layers: LayerSupport::all(),
            lights: true,
            cameras: true,
            points: true,
            lines: true,
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
            layers: LayerSupport::none(),
            lights: false,
            cameras: false,
            points: false,
            lines: false,
        }
    }

    /// Whether primitives of `topology` survive: triangles always, lines and
    /// points as `lines` and `points` say.
    pub const fn holds(&self, topology: Topology) -> bool {
        match topology {
            Topology::Triangles => true,
            Topology::Lines => self.lines,
            Topology::Points => self.points,
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
            layers: self.layers.intersect(other.layers),
            lights: self.lights && other.lights,
            cameras: self.cameras && other.cameras,
            points: self.points && other.points,
            lines: self.lines && other.lines,
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
    /// Images in an encoding the target does not accept: a writer drops and
    /// reports a WebP or KTX2 image in every container but glb and gltf,
    /// which carry them through `EXT_texture_webp` and `KHR_texture_basisu`.
    /// A caller that runs `normalize::transcode_textures` first hands the
    /// writer PNG instead, and is left only the images that call could not
    /// decode or left out of its texture budget (its `TranscodeReport`).
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
    /// Punctual lights.
    Lights,
    /// Cameras.
    Cameras,
    /// Point primitives: a point cloud, or the points beside a model.
    Points,
    /// Line primitives: a wireframe, measurement lines, hair guides.
    Lines,
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
            LossKind::Lights => "lights",
            LossKind::Cameras => "cameras",
            LossKind::Points => "points",
            LossKind::Lines => "lines",
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
///
/// The points and lines a target cannot hold are named by their count of
/// primitives; everything else is judged on the primitives it keeps.
pub fn losses(scene: &Scene, caps: &Capabilities, target: &str) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push =
        |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("{target} {why}")));

    // The primitives the target keeps: points and lines it lacks are named
    // below, once, and are not judged again under the other kinds.
    let prims: Vec<&Primitive> =
        scene.meshes.iter().flat_map(|m| &m.primitives).filter(|p| caps.holds(p.topology)).collect();
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
    // A map the target keeps, read through a uv set it drops, goes with it.
    // The target keeps the first `caps.uv_sets` sets of each material's order
    // (`pose::UvSetPlan`, which `pose::degrade` applies): the sets of the maps
    // its material model keeps come first, wherever cutting to the first sets
    // would drop one of them. A map the target drops anyway
    // (metallic-roughness and occlusion under Phong, all but base colour under
    // colour-only, and a layer's where it lacks the layer) is not counted,
    // wherever its set sits: the materials loss below already names it. That
    // changes the report only for a scene with such a map on a later set,
    // which is named once, under materials.
    let plan = super::pose::UvSetPlan::for_caps(scene, caps);
    let past = plan.planned_binding_sets(scene).filter(|&set| set as usize >= caps.uv_sets).count();
    if caps.textures && past > 0 {
        push(
            LossKind::Textures,
            format!("{past} texture binding(s) on uv set {} or later", caps.uv_sets),
            "drops those uv sets, so the textures they map are dropped too",
        );
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
    // A layer the target lacks goes whole, with its maps; the base layer
    // under it stays, wherever the target keeps materials at all.
    for layer in MaterialLayer::ALL.into_iter().filter(|&l| !caps.layers.keeps(l)) {
        let n = scene.materials.iter().filter(|m| m.has_layer(layer)).count();
        if n > 0 {
            let name = layer.as_str();
            let why = match caps.materials {
                MaterialSupport::None => format!("has no {name} layer"),
                _ => format!("has no {name} layer; the base layer is kept"),
            };
            push(LossKind::Materials, format!("{name} on {n} material(s)"), &why);
        }
    }
    // A root litegen added itself is not the input's hierarchy.
    let own = scene.nodes.iter().filter(|n| !n.is_litegen_root()).count();
    if !caps.hierarchy && own > 1 {
        push(LossKind::Hierarchy, format!("{own} node(s) of hierarchy"), "is flat; transforms are baked into vertices");
    }
    if !caps.multiple_meshes && prims.len() > 1 {
        push(LossKind::Meshes, format!("{} separate primitive(s)", prims.len()), "holds one mesh; they are merged");
    }
    if caps.textures {
        // Sampling state only matters for textures that survive at all.
        let filtered = scene.textures.iter().filter(|t| !t.sampler.filters_by_default()).count();
        if !caps.texture_filters && filtered > 0 {
            push(
                LossKind::TextureSampling,
                format!("filter modes of {filtered} texture(s)"),
                "has no texture filter settings; readers filter their default way",
            );
        }
        let wrapped = scene.textures.iter().filter(|t| t.sampler.wrap_s != Wrap::Repeat || t.sampler.wrap_t != Wrap::Repeat).count();
        if !caps.texture_wrap && wrapped > 0 {
            push(LossKind::TextureSampling, format!("wrap modes of {wrapped} texture(s)"), "cannot hold them; readers repeat textures");
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
    if !caps.lights && !scene.lights.is_empty() {
        let names: Vec<String> =
            scene.lights.iter().enumerate().map(|(i, l)| l.name.clone().unwrap_or_else(|| format!("#{i}"))).collect();
        push(
            LossKind::Lights,
            format!("{} light(s) [{}]", names.len(), names.join(", ")),
            "cannot hold lights; readers light the model their own way",
        );
    }
    if !caps.cameras && !scene.cameras.is_empty() {
        let names: Vec<String> =
            scene.cameras.iter().enumerate().map(|(i, c)| c.name.clone().unwrap_or_else(|| format!("#{i}"))).collect();
        push(
            LossKind::Cameras,
            format!("{} camera(s) [{}]", names.len(), names.join(", ")),
            "cannot hold cameras; readers frame the model their own way",
        );
    }
    for (topology, kind, noun) in [(Topology::Points, LossKind::Points, "point"), (Topology::Lines, LossKind::Lines, "line")] {
        let n = scene.meshes.iter().flat_map(|m| &m.primitives).filter(|p| p.topology == topology).count();
        if !caps.holds(topology) && n > 0 {
            push(kind, format!("{n} {noun} primitive(s)"), &format!("cannot hold {noun}s; they are left out"));
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

    // ─── material layers ────────────────────────────────────────────────────

    /// Every slot, in `texture_refs` order.
    const EVERY_SLOT: [TextureSlot; 17] = [
        TextureSlot::BaseColor,
        TextureSlot::MetallicRoughness,
        TextureSlot::Normal,
        TextureSlot::Occlusion,
        TextureSlot::Emissive,
        TextureSlot::Clearcoat,
        TextureSlot::ClearcoatRoughness,
        TextureSlot::ClearcoatNormal,
        TextureSlot::Transmission,
        TextureSlot::VolumeThickness,
        TextureSlot::Specular,
        TextureSlot::SpecularColor,
        TextureSlot::SheenColor,
        TextureSlot::SheenRoughness,
        TextureSlot::Iridescence,
        TextureSlot::IridescenceThickness,
        TextureSlot::Anisotropy,
    ];

    /// Every layer at its extension's defaults, untextured.
    fn every_layer() -> Material {
        Material {
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
        }
    }

    /// [`one_node_scene`] drawn with `materials[0]`, one uv set, and one PNG
    /// texture (with as many more as `textures` asks) to bind.
    fn material_scene(materials: Vec<Material>, textures: usize) -> Scene {
        let mut s = one_node_scene();
        s.meshes[0].primitives[0].uvs = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]];
        s.meshes[0].primitives[0].material = Some(0);
        s.images.push(Image { name: None, format: ImageFormat::Png, data: Vec::new() });
        s.textures = (0..textures.max(1)).map(|_| Texture { name: None, image: 0, sampler: Sampler::default() }).collect();
        s.materials = materials;
        s
    }

    #[test]
    fn layer_textures_are_texture_refs() {
        // Slot i bound to texture i: `texture_refs` yields all seventeen, base
        // slots first, then each layer's in field order.
        let t = |i: usize| Some(TextureRef { texture: i, uv_set: 0, transform: None });
        let m = Material {
            base_color_texture: t(0),
            metallic_roughness_texture: t(1),
            normal_texture: t(2),
            occlusion_texture: t(3),
            emissive_texture: t(4),
            clearcoat: Some(Clearcoat { texture: t(5), roughness_texture: t(6), normal_texture: t(7), ..Default::default() }),
            transmission: Some(Transmission { texture: t(8), ..Default::default() }),
            volume: Some(Volume { thickness_texture: t(9), ..Default::default() }),
            specular: Some(Specular { texture: t(10), color_texture: t(11), ..Default::default() }),
            sheen: Some(Sheen { color_texture: t(12), roughness_texture: t(13), ..Default::default() }),
            iridescence: Some(Iridescence { texture: t(14), thickness_texture: t(15), ..Default::default() }),
            anisotropy: Some(Anisotropy { texture: t(16), ..Default::default() }),
            ..every_layer()
        };
        let refs: Vec<(TextureSlot, usize)> = m.texture_refs().map(|(slot, r)| (slot, r.texture)).collect();
        assert_eq!(refs, EVERY_SLOT.into_iter().zip(0..).collect::<Vec<_>>());
        let layer_slots: Vec<TextureSlot> = MaterialLayer::ALL.iter().flat_map(|l| l.texture_slots()).copied().collect();
        assert_eq!(layer_slots, EVERY_SLOT[5..], "each layer's slots, as `pose::kept_texture_slots` adds them");
        // The mutable list is kept by hand beside the shared one: it tags
        // every slot alike, in the same order.
        let tags: Vec<TextureSlot> = m.clone().texture_slots_mut().map(|(slot, _)| slot).collect();
        assert_eq!(tags, EVERY_SLOT, "texture_slots_mut's tags");
        // Mutably: the bound slots renumber, and every slot, layers too, drops.
        let mut renumbered = m.clone();
        for (_, r) in renumbered.texture_refs_mut() {
            r.texture += 100;
        }
        assert_eq!(renumbered.sheen.as_ref().and_then(|s| s.roughness_texture.as_ref()).map(|r| r.texture), Some(113));
        let mut bare = m.clone();
        assert_eq!(bare.texture_slots_mut().count(), 17);
        for (_, r) in bare.texture_slots_mut() {
            *r = None;
        }
        assert_eq!(bare, every_layer(), "every binding is gone, and every layer is still there");
        assert_eq!(Material::default().texture_slots_mut().count(), 5, "a material without layers has the base slots alone");
        // A layer's binding is checked like a base one.
        let mut s = material_scene(vec![m], 17);
        s.validate().unwrap();
        s.materials[0].sheen.as_mut().unwrap().color_texture = t(17);
        match s.validate() {
            Err(MeshError::Malformed(e)) => assert!(e.contains("sheenColor texture 17"), "{e}"),
            other => panic!("{other:?}"),
        }
        // Specular and sheen colours are sRGB, as base colour and emission are.
        let colour: Vec<TextureSlot> = EVERY_SLOT.into_iter().filter(|s| s.is_color()).collect();
        assert_eq!(colour, [TextureSlot::BaseColor, TextureSlot::Emissive, TextureSlot::SpecularColor, TextureSlot::SheenColor]);
    }

    #[test]
    fn layer_factors_must_be_finite() {
        let validates = |m: Material| material_scene(vec![m], 1).validate();
        // Every layer at its defaults is valid, attenuation at +inf included.
        assert_eq!(every_layer().volume.unwrap().attenuation_distance, f32::INFINITY);
        validates(every_layer()).unwrap();
        let spoilt: [(&str, fn(&mut Material)); 19] = [
            ("clearcoat factor", |m| m.clearcoat.as_mut().unwrap().factor = f32::NAN),
            ("clearcoat roughness", |m| m.clearcoat.as_mut().unwrap().roughness = f32::INFINITY),
            ("clearcoat normal scale", |m| m.clearcoat.as_mut().unwrap().normal_scale = f32::NEG_INFINITY),
            ("transmission", |m| m.transmission.as_mut().unwrap().factor = f32::NAN),
            ("volume thickness", |m| m.volume.as_mut().unwrap().thickness = f32::INFINITY),
            ("attenuation distance NaN", |m| m.volume.as_mut().unwrap().attenuation_distance = f32::NAN),
            ("attenuation distance -inf", |m| m.volume.as_mut().unwrap().attenuation_distance = f32::NEG_INFINITY),
            ("attenuation colour", |m| m.volume.as_mut().unwrap().attenuation_color[2] = f32::NAN),
            ("specular factor", |m| m.specular.as_mut().unwrap().factor = f32::NAN),
            ("specular colour", |m| m.specular.as_mut().unwrap().color[0] = f32::INFINITY),
            ("sheen colour", |m| m.sheen.as_mut().unwrap().color[1] = f32::NAN),
            ("sheen roughness", |m| m.sheen.as_mut().unwrap().roughness = f32::NAN),
            ("iridescence factor", |m| m.iridescence.as_mut().unwrap().factor = f32::NAN),
            ("iridescence ior", |m| m.iridescence.as_mut().unwrap().ior = f32::INFINITY),
            ("iridescence thickness minimum", |m| m.iridescence.as_mut().unwrap().thickness_min = f32::NAN),
            ("iridescence thickness maximum", |m| m.iridescence.as_mut().unwrap().thickness_max = f32::INFINITY),
            ("anisotropy strength", |m| m.anisotropy.as_mut().unwrap().strength = f32::NAN),
            ("anisotropy rotation", |m| m.anisotropy.as_mut().unwrap().rotation = f32::NEG_INFINITY),
            ("dispersion", |m| m.dispersion = Some(f32::NAN)),
        ];
        for (what, spoil) in spoilt {
            let mut m = every_layer();
            spoil(&mut m);
            assert!(matches!(validates(m), Err(MeshError::Malformed(_))), "{what}");
        }
    }

    /// The bounds of KHR_materials_ior's schema,
    /// https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_materials_ior/schema/material.KHR_materials_ior.schema.json:
    /// `"oneOf": [{ "minimum": 0.0, "maximum": 0.0 }, { "minimum": 1.0 }]`,
    /// of a JSON number, which is finite.
    #[test]
    fn ior_outside_the_schema_is_refused() {
        let validates = |ior: f32| material_scene(vec![Material { ior: Some(ior), ..Default::default() }], 1).validate();
        for ok in [0.0, -0.0, 1.0, 1.5, 2.42, 1000.0] {
            validates(ok).unwrap_or_else(|e| panic!("ior {ok}: {e}"));
        }
        for bad in [f32::MIN_POSITIVE, 0.5, 1.0 - f32::EPSILON, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(matches!(validates(bad), Err(MeshError::Malformed(_))), "ior {bad}");
        }
    }

    #[test]
    fn losses_name_each_missing_layer() {
        // Every layer on one material, clearcoat and ior on a second, nothing
        // on a third.
        let s = material_scene(
            vec![every_layer(), Material { clearcoat: Some(Clearcoat::default()), ior: Some(1.4), ..Default::default() }, Material::default()],
            1,
        );
        let no_layers = Capabilities { layers: LayerSupport::none(), ..Capabilities::full() };
        let got: Vec<(LossKind, String, String)> = losses(&s, &no_layers, "usdz").into_iter().map(|l| (l.kind, l.what, l.why)).collect();
        let want: Vec<(LossKind, String, String)> = [
            ("clearcoat", 2),
            ("transmission", 1),
            ("volume", 1),
            ("ior", 2),
            ("specular", 1),
            ("sheen", 1),
            ("iridescence", 1),
            ("anisotropy", 1),
            ("dispersion", 1),
        ]
        .into_iter()
        .map(|(layer, n)| (LossKind::Materials, format!("{layer} on {n} material(s)"), format!("usdz has no {layer} layer; the base layer is kept")))
        .collect();
        assert_eq!(got, want);
        // A trip through a container that keeps clearcoat and ior names the
        // other seven.
        let some = Capabilities { layers: LayerSupport { clearcoat: true, ior: true, ..LayerSupport::none() }, ..Capabilities::full() };
        let named: Vec<String> = losses(&s, &Capabilities::full().intersect(some), "usdz").into_iter().map(|l| l.what).collect();
        assert_eq!(
            named,
            [
                "transmission on 1 material(s)",
                "volume on 1 material(s)",
                "specular on 1 material(s)",
                "sheen on 1 material(s)",
                "iridescence on 1 material(s)",
                "anisotropy on 1 material(s)",
                "dispersion on 1 material(s)",
            ]
        );
        assert!(losses(&s, &Capabilities::full(), "glb").is_empty());
        // A container with no materials at all keeps no base layer either.
        let stl = losses(&s, &Capabilities::none(), "stl");
        let coat = stl.iter().find(|l| l.what == "clearcoat on 2 material(s)").unwrap_or_else(|| panic!("{stl:?}"));
        assert_eq!((coat.kind, coat.why.as_str()), (LossKind::Materials, "stl has no clearcoat layer"));
    }

    #[test]
    fn layer_textures_on_dropped_uv_sets_are_lost() {
        // Base colour on set 0, occlusion on 1 and a clearcoat map on 2, into
        // a container of two sets: a layer it keeps loses the map with the
        // set, which the uv-set loss counts.
        let on = |uv_set: u32| Some(TextureRef { texture: 0, uv_set, transform: None });
        let mut s = material_scene(
            vec![Material {
                base_color_texture: on(0),
                occlusion_texture: on(1),
                clearcoat: Some(Clearcoat { factor: 1.0, texture: on(2), ..Default::default() }),
                ..Default::default()
            }],
            1,
        );
        s.meshes[0].primitives[0].uvs = vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]; 3];
        let keeps = Capabilities { uv_sets: 2, layers: LayerSupport { clearcoat: true, ..LayerSupport::none() }, ..Capabilities::full() };
        let what: Vec<String> = losses(&s, &keeps, "usdz").into_iter().map(|l| l.what).collect();
        assert_eq!(what, ["1 of 3 uv set(s)", "1 texture binding(s) on uv set 2 or later"]);
        // A container without the layer names it once, as the layer, not
        // again as a binding on a set it drops.
        let lacks = Capabilities { layers: LayerSupport::none(), ..keeps };
        let what: Vec<String> = losses(&s, &lacks, "usdz").into_iter().map(|l| l.what).collect();
        assert_eq!(what, ["1 of 3 uv set(s)", "clearcoat on 1 material(s)"]);
    }

    #[test]
    fn layer_transforms_conflict_like_base_ones() {
        // An untransformed base colour and a shifted sheen colour map on set
        // 0: only one can be baked into the set's coordinates.
        let shifted = Some(UvTransform { offset: [0.5, 0.0], rotation: 0.0, scale: [1.0, 1.0] });
        let mut s = material_scene(
            vec![Material {
                base_color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: None }),
                sheen: Some(Sheen {
                    color: [1.0; 3],
                    color_texture: Some(TextureRef { texture: 0, uv_set: 0, transform: shifted }),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            1,
        );
        let baked = Capabilities { texture_transforms: false, ..Capabilities::full() };
        let what: Vec<String> = losses(&s, &baked, "usdz").into_iter().map(|l| l.what).collect();
        assert_eq!(what, ["texture transforms on 1 material(s)"]);
        // One shift on both is one transform, baked without loss.
        s.materials[0].base_color_texture.as_mut().unwrap().transform = shifted;
        assert!(losses(&s, &baked, "usdz").is_empty());
    }

    // ─── lights and cameras ─────────────────────────────────────────────────

    /// [`one_node_scene`] with a light of each kind and a camera of each
    /// projection, each placed by a node of its own under the mesh's: nodes
    /// 1 to 3 hold the lights, 4 and 5 the cameras.
    fn lit_scene() -> Scene {
        let mut s = one_node_scene();
        s.lights = vec![
            Light { name: Some("sun".into()), kind: LightKind::Directional, color: [1.0, 0.875, 0.75], intensity: 3.0, range: None },
            Light { name: Some("bulb".into()), kind: LightKind::Point, color: [1.0; 3], intensity: 20.0, range: Some(10.0) },
            Light { name: None, kind: LightKind::Spot { inner_cone: 0.2, outer_cone: 0.6 }, color: [0.0, 0.5, 1.0], intensity: 40.0, range: None },
        ];
        s.cameras = vec![
            Camera { name: Some("shot".into()), projection: Projection::Perspective { yfov: 0.8, aspect: Some(1.5), znear: 0.125, zfar: Some(64.0) } },
            Camera { name: None, projection: Projection::Orthographic { xmag: 3.0, ymag: 2.0, znear: 0.0625, zfar: 20.0 } },
        ];
        let lights = (0..3).map(|l| Node { light: Some(l), ..Default::default() });
        let cameras = (0..2).map(|c| Node { camera: Some(c), ..Default::default() });
        for node in lights.chain(cameras) {
            s.nodes.push(node);
            let i = s.nodes.len() - 1;
            s.nodes[0].children.push(i);
        }
        s
    }

    /// Every bound of `Light::check` and `Camera::check`, which
    /// `Scene::validate` applies to each light and camera: the spec's, and
    /// the minimums of glTF's own schemas, so that a scene that validates
    /// writes valid glTF (KHR_lights_punctual's `light.schema.json`: each
    /// colour component 0 to 1, an intensity of 0 or more, a range over 0;
    /// glTF 2.0's `camera.perspective.schema.json`: an aspect ratio over 0).
    /// The reason names the field each refused value breaks, and the value.
    #[test]
    fn light_and_camera_values_are_validated() {
        lit_scene().validate().unwrap();
        let light = |kind: LightKind, color: [f32; 3], intensity: f32, range: Option<f32>| Light { name: None, kind, color, intensity, range };
        let spot = |inner_cone: f32, outer_cone: f32| LightKind::Spot { inner_cone, outer_cone };
        let persp = |yfov: f32, aspect: Option<f32>, znear: f32, zfar: Option<f32>| Camera {
            name: None,
            projection: Projection::Perspective { yfov, aspect, znear, zfar },
        };
        let ortho = |xmag: f32, ymag: f32, znear: f32, zfar: f32| Camera { name: None, projection: Projection::Orthographic { xmag, ymag, znear, zfar } };
        let (white, point) = ([1.0; 3], LightKind::Point);

        // One per bound: the field its reason names, the value, and the light.
        let bad_lights = [
            ("colour", "NaN", light(point, [1.0, f32::NAN, 1.0], 20.0, None)),
            ("colour", "1.5", light(point, [1.5, 1.0, 1.0], 20.0, None)),
            ("colour", "-0.25", light(point, [1.0, 1.0, -0.25], 20.0, None)),
            ("intensity", "inf", light(point, white, f32::INFINITY, None)),
            ("intensity", "-1", light(LightKind::Directional, white, -1.0, None)),
            ("range", "inf", light(point, white, 20.0, Some(f32::INFINITY))),
            ("range", "0", light(point, white, 20.0, Some(0.0))),
            ("inner cone", "NaN", light(spot(f32::NAN, 0.6), white, 40.0, None)),
            ("inner cone", "-0.1", light(spot(-0.1, 0.6), white, 40.0, None)),
            ("inner cone", "0.6", light(spot(0.6, 0.6), white, 40.0, None)),
            ("inner cone", "0.7", light(spot(0.7, 0.6), white, 40.0, None)),
            ("outer cone", "inf", light(spot(0.2, f32::INFINITY), white, 40.0, None)),
            ("outer cone", "1.6", light(spot(0.2, 1.6), white, 40.0, None)),
        ];
        for (field, value, bad) in bad_lights {
            let Err(why) = bad.check() else { panic!("a light with {field} {value} passed") };
            assert!(why.contains(field) && why.contains(value), "{field} {value}: {why}");
            let mut s = lit_scene();
            s.lights[1] = bad;
            assert!(matches!(s.validate(), Err(MeshError::Malformed(_))), "{field} {value}");
        }
        let bad_cameras = [
            ("yfov", "NaN", persp(f32::NAN, Some(1.5), 0.125, Some(64.0))),
            ("yfov", "0", persp(0.0, Some(1.5), 0.125, Some(64.0))),
            ("yfov", "3.1415927", persp(std::f32::consts::PI, Some(1.5), 0.125, Some(64.0))),
            ("aspect", "inf", persp(0.8, Some(f32::INFINITY), 0.125, Some(64.0))),
            ("aspect", "0", persp(0.8, Some(0.0), 0.125, Some(64.0))),
            ("znear", "inf", persp(0.8, Some(1.5), f32::INFINITY, None)),
            // A perspective znear must be over 0; an orthographic one may be 0.
            ("znear", "0", persp(0.8, Some(1.5), 0.0, Some(64.0))),
            ("zfar", "NaN", persp(0.8, Some(1.5), 0.125, Some(f32::NAN))),
            ("zfar", "0.125", persp(0.8, Some(1.5), 0.125, Some(0.125))),
            ("xmag", "NaN", ortho(f32::NAN, 2.0, 0.0625, 20.0)),
            ("xmag", "0", ortho(0.0, 2.0, 0.0625, 20.0)),
            ("ymag", "0", ortho(3.0, 0.0, 0.0625, 20.0)),
            ("znear", "-0.5", ortho(3.0, 2.0, -0.5, 20.0)),
            ("zfar", "inf", ortho(3.0, 2.0, 0.0625, f32::INFINITY)),
            ("zfar", "0.0625", ortho(3.0, 2.0, 0.0625, 0.0625)),
        ];
        for (field, value, bad) in bad_cameras {
            let Err(why) = bad.check() else { panic!("a camera with {field} {value} passed") };
            assert!(why.contains(field) && why.contains(value), "{field} {value}: {why}");
            let mut s = lit_scene();
            s.cameras[0] = bad;
            assert!(matches!(s.validate(), Err(MeshError::Malformed(_))), "{field} {value}");
        }

        // The edges pass: an inner cone of 0, an outer cone of π/2 as f32
        // holds it, a black light of intensity 0; an orthographic znear of 0,
        // a perspective camera with neither aspect nor zfar, and a negative
        // magnification, which glTF advises against but allows.
        let fine_lights = [
            light(spot(0.0, 0.6), white, 40.0, None),
            light(spot(0.2, std::f32::consts::FRAC_PI_2), white, 40.0, Some(10.0)),
            light(LightKind::Directional, [0.0; 3], 0.0, None),
        ];
        for good in fine_lights {
            assert_eq!(good.check(), Ok(()), "{good:?}");
            let mut s = lit_scene();
            s.lights[1] = good;
            s.validate().unwrap();
        }
        let fine_cameras = [ortho(3.0, 2.0, 0.0, 20.0), persp(0.8, None, 0.125, None), ortho(-3.0, 2.0, 0.0625, 20.0)];
        for good in fine_cameras {
            assert_eq!(good.check(), Ok(()), "{good:?}");
            let mut s = lit_scene();
            s.cameras[0] = good;
            s.validate().unwrap();
        }

        // A node placing a light or camera the scene does not have.
        let mut s = lit_scene();
        s.nodes[1].light = Some(3);
        match s.validate() {
            Err(MeshError::Malformed(e)) => assert!(e.contains("light 3"), "{e}"),
            other => panic!("{other:?}"),
        }
        let mut s = lit_scene();
        s.nodes[4].camera = Some(2);
        match s.validate() {
            Err(MeshError::Malformed(e)) => assert!(e.contains("camera 2"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn losses_name_lights_and_cameras() {
        let s = lit_scene();
        let lacks = Capabilities { lights: false, cameras: false, ..Capabilities::full() };
        let got: Vec<(LossKind, String, String)> = losses(&s, &lacks, "usdz").into_iter().map(|l| (l.kind, l.what, l.why)).collect();
        let want = [
            (LossKind::Lights, "3 light(s) [sun, bulb, #2]", "usdz cannot hold lights; readers light the model their own way"),
            (LossKind::Cameras, "2 camera(s) [shot, #1]", "usdz cannot hold cameras; readers frame the model their own way"),
        ]
        .map(|(kind, what, why)| (kind, what.to_string(), why.to_string()));
        assert_eq!(got, want);
        // Each is named alone where it alone goes, through a trip too, and
        // neither where both stay or where the scene has none.
        let kinds = |s: &Scene, caps: Capabilities| -> Vec<LossKind> { losses(s, &caps, "usdz").into_iter().map(|l| l.kind).collect() };
        assert_eq!(kinds(&s, Capabilities { cameras: false, ..Capabilities::full() }), [LossKind::Cameras]);
        assert_eq!(kinds(&s, Capabilities::full().intersect(Capabilities { lights: false, ..Capabilities::full() })), [LossKind::Lights]);
        assert!(kinds(&s, Capabilities::full()).is_empty());
        assert!(kinds(&one_node_scene(), lacks).is_empty());
    }

    // ─── points and lines ───────────────────────────────────────────────────

    /// A primitive of `topology` over `n` vertices in a row, drawing `indices`.
    fn drawing(topology: Topology, n: usize, indices: &[u32]) -> Primitive {
        Primitive {
            positions: (0..n).map(|i| [i as f32, 0.0, 0.0]).collect(),
            indices: indices.to_vec(),
            topology,
            ..Default::default()
        }
    }

    /// A scene of `prims`, each in a mesh and a root node of its own.
    fn scene_of(prims: Vec<Primitive>) -> Scene {
        let mut s = Scene::default();
        for p in prims {
            s.meshes.push(SceneMesh { primitives: vec![p], ..Default::default() });
            let mesh = s.meshes.len() - 1;
            s.add_root(Node { mesh: Some(mesh), ..Default::default() });
        }
        s
    }

    /// [`scene_of`] a triangle and `prims`.
    fn beside_a_triangle(prims: Vec<Primitive>) -> Scene {
        scene_of(std::iter::once(tri_prim()).chain(prims).collect())
    }

    /// `validate`'s verdict on one primitive of `topology` over 4 vertices,
    /// beside a triangle: its reason when it refuses.
    fn verdict(topology: Topology, indices: &[u32]) -> Result<(), String> {
        match beside_a_triangle(vec![drawing(topology, 4, indices)]).validate() {
            Ok(()) => Ok(()),
            Err(MeshError::Malformed(why)) => Err(why),
            Err(other) => panic!("{topology:?} {indices:?}: {other:?}"),
        }
    }

    /// Every segment is two indices, so a line list takes pairs and a
    /// triangle list still takes threes.
    #[test]
    fn a_lines_primitive_needs_pairs() {
        for indices in [&[][..], &[0, 1], &[0, 1, 1, 2], &[0, 1, 1, 2, 2, 3, 3, 0]] {
            assert_eq!(verdict(Topology::Lines, indices), Ok(()), "{indices:?}");
        }
        for indices in [&[0][..], &[0, 1, 2], &[0, 1, 2, 3, 0]] {
            let why = verdict(Topology::Lines, indices).expect_err("an odd number of indices is half a segment");
            assert!(why.contains(&format!("index count {} is not a multiple of 2", indices.len())), "{why}");
        }
        // The same count is a whole triangle list, and four are not.
        assert_eq!(verdict(Topology::Triangles, &[0, 1, 2]), Ok(()));
        let why = verdict(Topology::Triangles, &[0, 1, 2, 3]).expect_err("four indices");
        assert!(why.contains("not a multiple of 3"), "{why}");
        // Whatever it draws, every index must name a vertex.
        let why = verdict(Topology::Lines, &[0, 4]).expect_err("vertex 4 of 4");
        assert!(why.contains("index 4 out of range for 4 vertices"), "{why}");
    }

    /// A point is one index, so a point list takes any count, but every index
    /// must still name a vertex.
    #[test]
    fn points_take_any_count() {
        for indices in [&[][..], &[0], &[0, 1], &[3, 2, 1], &[0, 1, 2, 3], &[0, 1, 2, 3, 0]] {
            assert_eq!(verdict(Topology::Points, indices), Ok(()), "{indices:?}");
        }
        let why = verdict(Topology::Points, &[0, 1, 9]).expect_err("vertex 9 of 4");
        assert!(why.contains("index 9 out of range for 4 vertices"), "{why}");
    }

    /// The scene caps charge every topology's indices: a scene's budget is
    /// its indices, drawn as triangles, segments or points.
    #[test]
    fn the_scene_caps_count_every_topologys_indices() {
        let p = |n: usize| Primitive { positions: vec![[0.0; 3]], indices: vec![0; n], topology: Topology::Points, ..Default::default() };
        let mut s = beside_a_triangle(vec![p(MAX_SCENE_INDICES)]);
        match s.validate() {
            Err(MeshError::Unsupported(why)) => assert!(why.contains("indices, over the"), "{why}"),
            other => panic!("the triangle's 3 indices and the points' {MAX_SCENE_INDICES} are over the cap: {other:?}"),
        }
        s.meshes[1].primitives[0].indices.truncate(MAX_SCENE_INDICES - 3);
        s.validate().unwrap();
    }

    /// `triangle_count` counts triangles alone and `drawable_count` counts
    /// what is drawn, by element: each triangle, each segment, each point.
    /// Only a scene with none is empty.
    #[test]
    fn a_scene_of_points_is_not_empty() {
        use super::super::{require_drawable, write_scene, MeshFormat, WriteOptions};
        let counts = |s: &Scene| (s.triangle_count(), s.drawable_count());
        let only_points = scene_of(vec![drawing(Topology::Points, 9, &(0..9).collect::<Vec<u32>>())]);
        assert_eq!(counts(&only_points), (0, 9), "nine points");
        let lines = drawing(Topology::Lines, 4, &[0, 1, 1, 2, 2, 3, 3, 0]);
        assert_eq!(counts(&beside_a_triangle(vec![lines.clone()])), (1, 5), "a triangle and four segments");
        let all = beside_a_triangle(vec![lines, drawing(Topology::Points, 5, &[0, 1, 2, 3, 4])]);
        assert_eq!(counts(&all), (1, 10), "a triangle, four segments and five points");
        let mut none = all.clone();
        for p in none.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
            p.indices.clear();
        }
        assert_eq!(counts(&none), (0, 0));

        // The readers' and writers' gate, which a file of points passes and a
        // scene that draws nothing does not.
        assert_eq!(require_drawable(&only_points), Ok(()), "nine points are drawn");
        assert_eq!(require_drawable(&all), Ok(()));
        assert_eq!(require_drawable(&none), Err(MeshError::Empty), "nothing is");
        assert_eq!(write_scene(&none, MeshFormat::Glb, WriteOptions::default()).unwrap_err(), MeshError::Empty);
        assert_eq!(MeshError::Empty.to_string(), "mesh contains no triangles, lines or points");
    }

    /// Triangles survive every container, lines and points as the flags say,
    /// and a trip through two containers keeps what both hold.
    #[test]
    fn capabilities_hold_points_and_lines_as_their_flags_say() {
        let (full, none) = (Capabilities::full(), Capabilities::none());
        assert_eq!((full.points, full.lines, none.points, none.lines), (true, true, false, false));
        for t in [Topology::Triangles, Topology::Lines, Topology::Points] {
            assert!(full.holds(t), "{t:?}");
            assert_eq!(none.holds(t), t == Topology::Triangles, "{t:?}");
        }
        let only_points = Capabilities { points: true, lines: false, ..none };
        assert_eq!((only_points.holds(Topology::Points), only_points.holds(Topology::Lines)), (true, false));
        let both = full.intersect(only_points);
        assert_eq!((both.points, both.lines), (true, false));
        let neither = only_points.intersect(Capabilities { points: false, lines: true, ..none });
        assert_eq!((neither.points, neither.lines), (false, false));
    }

    fn what_and_why(s: &Scene, caps: &Capabilities, kind: LossKind) -> Vec<(String, String)> {
        losses(s, caps, "stl").into_iter().filter(|l| l.kind == kind).map(|l| (l.what, l.why)).collect()
    }

    /// A container without points names them, by the points primitives the
    /// scene has, and not otherwise.
    #[test]
    fn losses_name_points_by_count() {
        let points = |n: usize| drawing(Topology::Points, n, &(0..n as u32).collect::<Vec<_>>());
        let s = beside_a_triangle(vec![points(9), points(4), points(1)]);
        let lacks = Capabilities { points: false, ..Capabilities::full() };
        assert_eq!(
            what_and_why(&s, &lacks, LossKind::Points),
            [("3 point primitive(s)".to_string(), "stl cannot hold points; they are left out".to_string())]
        );
        assert_eq!(LossKind::Points.as_str(), "points");
        // Not lines, which it holds; not where it holds points; not where the
        // scene has none, or has lines alone.
        assert!(what_and_why(&s, &lacks, LossKind::Lines).is_empty());
        assert!(what_and_why(&s, &Capabilities { lines: false, ..Capabilities::full() }, LossKind::Points).is_empty());
        assert!(losses(&s, &Capabilities::full(), "stl").is_empty());
        assert!(losses(&one_node_scene(), &Capabilities::none(), "stl").is_empty());
        let lines = beside_a_triangle(vec![drawing(Topology::Lines, 2, &[0, 1])]);
        assert!(what_and_why(&lines, &Capabilities::none(), LossKind::Points).is_empty());
    }

    /// A container without lines names them as one without points names
    /// points, the two together in that order, and neither the other's.
    #[test]
    fn losses_name_lines_by_count() {
        let s = beside_a_triangle(vec![drawing(Topology::Lines, 4, &[0, 1, 1, 2, 2, 3, 3, 0]), drawing(Topology::Lines, 2, &[0, 1])]);
        let lacks = Capabilities { lines: false, ..Capabilities::full() };
        assert_eq!(
            what_and_why(&s, &lacks, LossKind::Lines),
            [("2 line primitive(s)".to_string(), "stl cannot hold lines; they are left out".to_string())]
        );
        assert_eq!(LossKind::Lines.as_str(), "lines");
        assert!(what_and_why(&s, &Capabilities { points: false, ..Capabilities::full() }, LossKind::Lines).is_empty());
        assert!(losses(&s, &Capabilities::full(), "stl").is_empty());

        let mut both = s.clone();
        both.meshes.push(SceneMesh { primitives: vec![drawing(Topology::Points, 1, &[0])], ..Default::default() });
        both.add_root(Node { mesh: Some(both.meshes.len() - 1), ..Default::default() });
        let bare = Capabilities { hierarchy: true, ..Capabilities::none() };
        let kinds: Vec<LossKind> = losses(&both, &bare, "stl").into_iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [LossKind::Points, LossKind::Lines]);
    }

    /// Everything else a scene loses is judged on the primitives the target
    /// keeps: what only a dropped points or lines primitive has (colours,
    /// normals, uv sets, being separate) is named once, under `points` or
    /// `lines`, and not again under another kind.
    #[test]
    fn losses_judge_the_rest_on_what_the_target_keeps() {
        let dressed = |topology: Topology| Primitive {
            normals: Some(vec![[0.0, 0.0, 1.0]; 2]),
            uvs: vec![vec![[0.0; 2]; 2]; 2],
            colors: Some(vec![[1.0; 4]; 2]),
            ..drawing(topology, 2, &[0, 1])
        };
        let s = beside_a_triangle(vec![dressed(Topology::Lines), dressed(Topology::Points)]);
        let kinds = |caps: &Capabilities| -> Vec<LossKind> { losses(&s, caps, "ply").into_iter().map(|l| l.kind).collect() };
        // A container of triangles in one mesh, with nothing else to hold.
        let bare = Capabilities { hierarchy: true, ..Capabilities::none() };
        assert_eq!(kinds(&bare), [LossKind::Points, LossKind::Lines]);
        // One that holds the points holds their colours, and reports what it
        // would lose of them: normals, and uv sets, and being separate.
        let keeps_points = Capabilities { points: true, ..bare };
        assert_eq!(kinds(&keeps_points), [LossKind::Normals, LossKind::Uvs, LossKind::VertexColors, LossKind::Meshes, LossKind::Lines]);
        let what = |kind: LossKind| -> String { losses(&s, &keeps_points, "ply").into_iter().find(|l| l.kind == kind).unwrap().what };
        assert_eq!(what(LossKind::Meshes), "2 separate primitive(s)", "the triangle and the points");
        assert_eq!(what(LossKind::Uvs), "2 of 2 uv set(s)");
    }

    /// `blender_energy` is the `energy` Blender's glTF importer gives a
    /// light: a 3 lux sun gets 3 / 683, and a 100 cd point or spot light
    /// 100 × 4π / 683 (a spot's cone changes nothing), with the numbers
    /// Task 1 probed in Blender 5.2.0 for `staged` (3 lux: 0.0043923864;
    /// 20 cd: 0.36797571; 40 cd: 0.73595142). `from_blender_energy` inverts
    /// each to 1e-12 relative, across the range of an f32.
    #[test]
    fn blender_energy_is_blenders_gltf_import() {
        use std::f64::consts::PI;
        let rel = |got: f64, want: f64| (got - want).abs() / want.abs();
        let spot = |outer_cone: f32| LightKind::Spot { inner_cone: 0.0, outer_cone };
        assert_eq!(BLENDER_LUMENS_PER_WATT, 683.0);
        assert!(rel(blender_energy(&LightKind::Directional, 3.0), 3.0 / 683.0) < 1e-15);
        assert!(rel(blender_energy(&LightKind::Directional, 3.0), 0.0043923864) < 1e-7, "{}", blender_energy(&LightKind::Directional, 3.0));
        for kind in [LightKind::Point, spot(0.1), spot(1.5)] {
            assert!(rel(blender_energy(&kind, 100.0), 100.0 * 4.0 * PI / 683.0) < 1e-15, "{kind:?}");
            assert!(rel(blender_energy(&kind, 20.0), 0.36797571) < 1e-7, "{kind:?}: {}", blender_energy(&kind, 20.0));
            assert!(rel(blender_energy(&kind, 40.0), 0.73595142) < 1e-7, "{kind:?}: {}", blender_energy(&kind, 40.0));
            assert!(rel(from_blender_energy(&kind, 1.0), 683.0 / (4.0 * PI)) < 1e-15, "{kind:?}");
        }
        assert_eq!(from_blender_energy(&LightKind::Directional, 1.0), 683.0);
        for kind in [LightKind::Directional, LightKind::Point, spot(0.6)] {
            for value in [1e-45, 1e-30, 0.0044, 1.0, 3.0, 100.0, 1e6, 3.4e38] {
                let back = from_blender_energy(&kind, blender_energy(&kind, value));
                assert!(rel(back, value) < 1e-12, "{kind:?}: {value} came back as {back}");
            }
            assert_eq!(blender_energy(&kind, 0.0), 0.0, "{kind:?}");
        }
    }
}
