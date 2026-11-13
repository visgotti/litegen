//! What the converter's reader measured in a model (mesh's `inspect`): the
//! counts, bounds, materials, skin and clips a viewer or a caller shows.

use litegen_mesh::scene::TextureSlot;
use litegen_mesh::{inspect, Scene};

use crate::Error;

/// What a read model holds, measured by the converter's own reader.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelInfo {
    /// Triangles in the model's meshes. A mesh that several nodes place is
    /// counted once.
    pub triangles: u32,
    /// Vertices in the model's meshes, each mesh counted once.
    pub vertices: u32,
    /// Nodes in the scene graph, joints included.
    pub nodes: u32,
    /// Meshes, each counted once however many nodes place it.
    pub meshes: u32,
    /// Images the materials use.
    pub textures: u32,
    /// Most uv sets on any primitive.
    pub uv_sets: u32,
    /// Whether any primitive has vertex colours.
    pub vertex_colors: bool,
    /// World-space bounds at rest, in metres (the bind pose for a skinned
    /// model), every instance placed. `None` for a model with no positions to
    /// bound.
    pub bounds: Option<Bounds>,
    /// Every material, in the file's order.
    pub materials: Vec<MaterialInfo>,
    /// The first skin, when the model has one.
    pub skin: Option<SkinInfo>,
    /// Every animation clip.
    pub animations: Vec<AnimationInfo>,
}

/// An axis-aligned box, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    /// The smallest x, y and z.
    pub min: [f64; 3],
    /// The largest x, y and z.
    pub max: [f64; 3],
}

/// One material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialInfo {
    /// Its name, when the file gives one.
    pub name: Option<String>,
    /// The texture maps it uses, in slot order.
    pub maps: Vec<TextureMap>,
}

/// A model's skin: the joints that deform its mesh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinInfo {
    /// How many joints it has.
    pub joints: u32,
    /// The joint every other joint descends from.
    pub root: Option<String>,
    /// Its rest pose.
    pub bind_pose: BindPose,
    /// Which joint is which humanoid bone, when the joints' names follow a
    /// naming scheme litegen knows.
    pub humanoid: Option<Humanoid>,
}

/// A humanoid rig's bones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Humanoid {
    /// The naming scheme its joints follow.
    pub naming: HumanoidNaming,
    /// Each VRM 1.0 humanoid bone the rig has, in VRM's order, and the joint
    /// that is it.
    pub bones: Vec<HumanoidBone>,
}

/// One humanoid bone and the joint that is it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanoidBone {
    /// The bone, as VRM 1.0 names it.
    pub bone: VrmBone,
    /// The joint's name in the file.
    pub joint: String,
}

/// A closed set of names: the enum, `ALL` in order, `as_str` and `parse`.
macro_rules! named_set {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident = $text:literal,)+ }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($(#[$vdoc])* $variant,)+
        }

        impl $name {
            /// Every value, in order.
            pub const ALL: [$name; [$($text),+].len()] = [$($name::$variant),+];

            /// The value as litegen spells it.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text,)+
                }
            }

            /// The value `text` spells, exactly.
            pub fn parse(text: &str) -> Option<$name> {
                match text {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

named_set! {
    /// A material texture map: glTF's five core maps, and the layer maps of
    /// its PBR extensions.
    TextureMap {
        BaseColor = "base_color",
        MetallicRoughness = "metallic_roughness",
        Normal = "normal",
        Occlusion = "occlusion",
        Emissive = "emissive",
        Clearcoat = "clearcoat",
        ClearcoatRoughness = "clearcoat_roughness",
        ClearcoatNormal = "clearcoat_normal",
        Transmission = "transmission",
        VolumeThickness = "volume_thickness",
        Specular = "specular",
        SpecularColor = "specular_color",
        SheenColor = "sheen_color",
        SheenRoughness = "sheen_roughness",
        Iridescence = "iridescence",
        IridescenceThickness = "iridescence_thickness",
        Anisotropy = "anisotropy",
    }
}

impl From<TextureSlot> for TextureMap {
    fn from(slot: TextureSlot) -> TextureMap {
        match slot {
            TextureSlot::BaseColor => TextureMap::BaseColor,
            TextureSlot::MetallicRoughness => TextureMap::MetallicRoughness,
            TextureSlot::Normal => TextureMap::Normal,
            TextureSlot::Occlusion => TextureMap::Occlusion,
            TextureSlot::Emissive => TextureMap::Emissive,
            TextureSlot::Clearcoat => TextureMap::Clearcoat,
            TextureSlot::ClearcoatRoughness => TextureMap::ClearcoatRoughness,
            TextureSlot::ClearcoatNormal => TextureMap::ClearcoatNormal,
            TextureSlot::Transmission => TextureMap::Transmission,
            TextureSlot::VolumeThickness => TextureMap::VolumeThickness,
            TextureSlot::Specular => TextureMap::Specular,
            TextureSlot::SpecularColor => TextureMap::SpecularColor,
            TextureSlot::SheenColor => TextureMap::SheenColor,
            TextureSlot::SheenRoughness => TextureMap::SheenRoughness,
            TextureSlot::Iridescence => TextureMap::Iridescence,
            TextureSlot::IridescenceThickness => TextureMap::IridescenceThickness,
            TextureSlot::Anisotropy => TextureMap::Anisotropy,
        }
    }
}

named_set! {
    /// A humanoid joint naming scheme litegen knows (mesh's humanoid tables).
    HumanoidNaming {
        /// VRM 1.0's own bone names.
        Vrm = "vrm",
        /// Unity's `HumanBodyBones`.
        Unity = "unity",
        /// Mixamo's `mixamorig:` joints.
        Mixamo = "mixamo",
        /// Meshy's rigging output.
        Meshy = "meshy",
        /// litegen's mock provider.
        LitegenMock = "litegen-mock",
    }
}

named_set! {
    /// A VRM 1.0 humanoid bone (`VRMC_vrm-1.0/humanoid.md`), optional bones
    /// included.
    VrmBone {
        Hips = "hips",
        Spine = "spine",
        Chest = "chest",
        UpperChest = "upperChest",
        Neck = "neck",
        Head = "head",
        LeftShoulder = "leftShoulder",
        LeftUpperArm = "leftUpperArm",
        LeftLowerArm = "leftLowerArm",
        LeftHand = "leftHand",
        RightShoulder = "rightShoulder",
        RightUpperArm = "rightUpperArm",
        RightLowerArm = "rightLowerArm",
        RightHand = "rightHand",
        LeftUpperLeg = "leftUpperLeg",
        LeftLowerLeg = "leftLowerLeg",
        LeftFoot = "leftFoot",
        LeftToes = "leftToes",
        RightUpperLeg = "rightUpperLeg",
        RightLowerLeg = "rightLowerLeg",
        RightFoot = "rightFoot",
        RightToes = "rightToes",
        LeftEye = "leftEye",
        RightEye = "rightEye",
        Jaw = "jaw",
        LeftThumbMetacarpal = "leftThumbMetacarpal",
        LeftThumbProximal = "leftThumbProximal",
        LeftThumbDistal = "leftThumbDistal",
        LeftIndexProximal = "leftIndexProximal",
        LeftIndexIntermediate = "leftIndexIntermediate",
        LeftIndexDistal = "leftIndexDistal",
        LeftMiddleProximal = "leftMiddleProximal",
        LeftMiddleIntermediate = "leftMiddleIntermediate",
        LeftMiddleDistal = "leftMiddleDistal",
        LeftRingProximal = "leftRingProximal",
        LeftRingIntermediate = "leftRingIntermediate",
        LeftRingDistal = "leftRingDistal",
        LeftLittleProximal = "leftLittleProximal",
        LeftLittleIntermediate = "leftLittleIntermediate",
        LeftLittleDistal = "leftLittleDistal",
        RightThumbMetacarpal = "rightThumbMetacarpal",
        RightThumbProximal = "rightThumbProximal",
        RightThumbDistal = "rightThumbDistal",
        RightIndexProximal = "rightIndexProximal",
        RightIndexIntermediate = "rightIndexIntermediate",
        RightIndexDistal = "rightIndexDistal",
        RightMiddleProximal = "rightMiddleProximal",
        RightMiddleIntermediate = "rightMiddleIntermediate",
        RightMiddleDistal = "rightMiddleDistal",
        RightRingProximal = "rightRingProximal",
        RightRingIntermediate = "rightRingIntermediate",
        RightRingDistal = "rightRingDistal",
        RightLittleProximal = "rightLittleProximal",
        RightLittleIntermediate = "rightLittleIntermediate",
        RightLittleDistal = "rightLittleDistal",
    }
}

/// A skinned humanoid's rest pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindPose {
    /// Arms within 15° of horizontal.
    T,
    /// Arms 25°–55° below horizontal.
    A,
    /// Neither, or no humanoid arms to measure.
    Unknown,
}

impl From<inspect::BindPose> for BindPose {
    fn from(p: inspect::BindPose) -> BindPose {
        match p {
            inspect::BindPose::T => BindPose::T,
            inspect::BindPose::A => BindPose::A,
            inspect::BindPose::Unknown => BindPose::Unknown,
        }
    }
}

impl BindPose {
    pub const ALL: [BindPose; 3] = [BindPose::T, BindPose::A, BindPose::Unknown];

    pub fn as_str(self) -> &'static str {
        match self {
            BindPose::T => "t",
            BindPose::A => "a",
            BindPose::Unknown => "unknown",
        }
    }
}

/// One animation clip.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationInfo {
    /// Its name, when the file gives one.
    pub name: Option<String>,
    /// How long it lasts, in seconds, from 0 to its last key.
    pub duration_s: f64,
    /// How far the root moves, first key to last, in metres.
    pub root_travel_m: [f64; 3],
    /// Whether the root stays where it starts, horizontally, all clip long.
    pub in_place: bool,
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// mesh's humanoid map, typed. Every table and bone mesh names has a typed
/// name (tests/read.rs `every_humanoid_table_and_bone_of_mesh_is_typed`).
fn humanoid((table, map): (&'static str, Vec<(String, String)>)) -> Option<Humanoid> {
    Some(Humanoid {
        naming: HumanoidNaming::parse(table)?,
        bones: map.into_iter().filter_map(|(bone, joint)| Some(HumanoidBone { bone: VrmBone::parse(&bone)?, joint })).collect(),
    })
}

/// The facts of `scene`. Measuring places every instance (mesh's
/// `inspect`), so it fails for a scene past mesh's merged-mesh cap.
pub(crate) fn of(scene: &Scene) -> Result<ModelInfo, Error> {
    let inspect::SceneFacts { bounds, vertex_count, triangle_count, uv_sets, vertex_colors, materials: _, skin, animations } =
        inspect::inspect(scene)?;
    Ok(ModelInfo {
        triangles: count(triangle_count),
        vertices: count(vertex_count),
        nodes: count(scene.nodes.len()),
        meshes: count(scene.meshes.len()),
        textures: count(scene.images.len()),
        uv_sets: count(uv_sets),
        vertex_colors,
        bounds: bounds.map(|(min, max)| Bounds { min: min.map(f64::from), max: max.map(f64::from) }),
        // inspect's own walk (`texture_refs`, slot by slot), typed through
        // an exhaustive match instead of its strings.
        materials: scene
            .materials
            .iter()
            .map(|m| MaterialInfo { name: m.name.clone(), maps: m.texture_refs().map(|(slot, _)| TextureMap::from(slot)).collect() })
            .collect(),
        skin: skin.map(|inspect::SkinFacts { joints, root, bind_pose, humanoid: map }| SkinInfo {
            joints: count(joints.len()),
            root,
            bind_pose: BindPose::from(bind_pose),
            humanoid: map.and_then(humanoid),
        }),
        animations: animations
            .into_iter()
            .map(|clip| {
                let in_place = clip.in_place();
                let inspect::ClipFacts { name, duration_s, root_travel_m, excursion_m: _ } = clip;
                AnimationInfo { name, duration_s: f64::from(duration_s), root_travel_m: root_travel_m.map(f64::from), in_place }
            })
            .collect(),
    })
}
