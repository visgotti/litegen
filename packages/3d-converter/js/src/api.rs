//! The TypeScript-facing types: serde + tsify mirrors of the core's, in
//! TypeScript's spellings (camelCase fields; the core's lowercase values).
//! tsify writes each one's declaration, doc comments included, into the
//! package's `.d.ts`. Every conversion from the core is an exhaustive match,
//! so a core addition fails this crate's build at the line to extend.

use litegen_3d_converter as core;
use serde::{Deserialize, Serialize};
use tsify::Tsify;

/// A string union mirroring one of the core's named sets: each value spelled
/// as the core spells it (checked by `closed_sets_spell_like_the_core`), and
/// an exhaustive conversion from the core's enum.
macro_rules! mirror {
    ($(#[$doc:meta])* $name:ident from $core:ident { $($(#[$vdoc:meta])* $variant:ident = $text:literal,)+ }) => {
        $(#[$doc])*
        #[derive(Tsify, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($(#[$vdoc])* #[serde(rename = $text)] $variant,)+
        }

        impl From<core::$core> for $name {
            fn from(value: core::$core) -> $name {
                match value {
                    $(core::$core::$variant => $name::$variant,)+
                }
            }
        }
    };
}

/// A 3D container the converter reads and writes. Every pair converts, both
/// ways. `usda` writes USDA text and `usdc` a binary crate; each reads either
/// encoding (`.usda`, `.usdc`, or a `.usd` holding either).
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    #[serde(rename = "glb")]
    Glb,
    #[serde(rename = "gltf")]
    Gltf,
    #[serde(rename = "obj")]
    Obj,
    #[serde(rename = "stl")]
    Stl,
    #[serde(rename = "ply")]
    Ply,
    #[serde(rename = "3mf")]
    ThreeMf,
    #[serde(rename = "dae")]
    Dae,
    #[serde(rename = "fbx")]
    Fbx,
    #[serde(rename = "usdz")]
    Usdz,
    #[serde(rename = "usda")]
    Usda,
    #[serde(rename = "usdc")]
    Usdc,
}

impl From<Format> for core::Format {
    fn from(f: Format) -> core::Format {
        match f {
            Format::Glb => core::Format::Glb,
            Format::Gltf => core::Format::Gltf,
            Format::Obj => core::Format::Obj,
            Format::Stl => core::Format::Stl,
            Format::Ply => core::Format::Ply,
            Format::ThreeMf => core::Format::ThreeMf,
            Format::Dae => core::Format::Dae,
            Format::Fbx => core::Format::Fbx,
            Format::Usdz => core::Format::Usdz,
            Format::Usda => core::Format::Usda,
            Format::Usdc => core::Format::Usdc,
        }
    }
}

impl From<core::Format> for Format {
    fn from(f: core::Format) -> Format {
        match f {
            core::Format::Glb => Format::Glb,
            core::Format::Gltf => Format::Gltf,
            core::Format::Obj => Format::Obj,
            core::Format::Stl => Format::Stl,
            core::Format::Ply => Format::Ply,
            core::Format::ThreeMf => Format::ThreeMf,
            core::Format::Dae => Format::Dae,
            core::Format::Fbx => Format::Fbx,
            core::Format::Usdz => Format::Usdz,
            core::Format::Usda => Format::Usda,
            core::Format::Usdc => Format::Usdc,
        }
    }
}

/// A length unit, for the containers with none of their own (STL, OBJ, PLY).
/// Slicers read STL as millimetres.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    M,
    Cm,
    Mm,
    In,
    Ft,
}

impl From<Unit> for core::Unit {
    fn from(u: Unit) -> core::Unit {
        match u {
            Unit::M => core::Unit::M,
            Unit::Cm => core::Unit::Cm,
            Unit::Mm => core::Unit::Mm,
            Unit::In => core::Unit::In,
            Unit::Ft => core::Unit::Ft,
        }
    }
}

/// A unitless file's up axis: `y` (glTF's) or `z` (slicers, CAD, Blender),
/// where +Y up becomes +Z up and the front (+Z) becomes −Y.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpAxis {
    Y,
    Z,
}

impl From<UpAxis> for core::UpAxis {
    fn from(a: UpAxis) -> core::UpAxis {
        match a {
            UpAxis::Y => core::UpAxis::Y,
            UpAxis::Z => core::UpAxis::Z,
        }
    }
}

/// How FBX material colours are stored: `srgb` (the default; three.js reads
/// them so) or `linear` (Blender, Unity, Unreal, ufbx and assimp do).
/// Vertex colours are sRGB either way.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FbxColorSpace {
    Srgb,
    Linear,
}

impl From<FbxColorSpace> for core::FbxColorSpace {
    fn from(c: FbxColorSpace) -> core::FbxColorSpace {
        match c {
            FbxColorSpace::Srgb => core::FbxColorSpace::Srgb,
            FbxColorSpace::Linear => core::FbxColorSpace::Linear,
        }
    }
}

/// How FBX and USD light intensities are stored: `photometric` (the default;
/// glTF's candela and lux, as ufbx and three.js read FBX) or `blender`
/// (Blender's watts, so the file opens in Blender as bright as the scene's
/// glTF does).
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LightUnits {
    Photometric,
    Blender,
}

impl From<LightUnits> for core::LightUnits {
    fn from(u: LightUnits) -> core::LightUnits {
        match u {
            LightUnits::Photometric => core::LightUnits::Photometric,
            LightUnits::Blender => core::LightUnits::Blender,
        }
    }
}

/// How a Collada file holds morph targets: `bake` (the default; each node's
/// mesh at that node's weights, which every Collada reader opens, with the
/// targets and their weight animation reported as losses) or `keep`
/// (`<morph>` controllers with their weight animation, which SceneKit (and so
/// Xcode) and Godot read, while three.js's ColladaLoader and assimp refuse
/// the whole file).
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DaeMorphs {
    Bake,
    Keep,
}

impl From<DaeMorphs> for core::DaeMorphs {
    fn from(m: DaeMorphs) -> core::DaeMorphs {
        match m {
            DaeMorphs::Bake => core::DaeMorphs::Bake,
            DaeMorphs::Keep => core::DaeMorphs::Keep,
        }
    }
}

/// What a USDZ package's root layer, its first entry, is: `usda` (the
/// default; USDA text, which `usdchecker --arkit` accepts and `<model-viewer>`
/// hands to AR Quick Look as it is) or `usdc` (the binary crate, as Apple's
/// own packagers write it). The two read back as one scene.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UsdzRoot {
    Usda,
    Usdc,
}

impl From<UsdzRoot> for core::UsdzRoot {
    fn from(r: UsdzRoot) -> core::UsdzRoot {
        match r {
            UsdzRoot::Usda => core::UsdzRoot::Usda,
            UsdzRoot::Usdc => core::UsdzRoot::Usdc,
        }
    }
}

/// Which animation clips a USD file holds: `all` (every clip as an
/// alternative, a variant set `Animation` on the default prim with one
/// variant per clip, the first selected, so a reader that composes variants
/// can switch between them and one that does not plays the first; rigid
/// (node) motion is the first clip's only, the others' is named in the
/// losses, and a scene whose clips are all rigid writes as `first`) or
/// `first` (the first clip alone). When absent, `all` for `usda` and `usdc`, and
/// `first` for `usdz` until AR Quick Look's behaviour with variants is
/// checked on a device.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UsdClips {
    All,
    First,
}

impl From<UsdClips> for core::UsdClips {
    fn from(c: UsdClips) -> core::UsdClips {
        match c {
            UsdClips::All => core::UsdClips::All,
            UsdClips::First => core::UsdClips::First,
        }
    }
}

/// What a conversion answers with: `file` (the default) is one
/// self-contained file; `zip` is one zip of the file and the sidecars it
/// references; `files` is those files separately, the model first.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Packaging {
    File,
    Zip,
    Files,
}

impl From<Packaging> for core::Packaging {
    fn from(p: Packaging) -> core::Packaging {
        match p {
            Packaging::File => core::Packaging::File,
            Packaging::Zip => core::Packaging::Zip,
            Packaging::Files => core::Packaging::Files,
        }
    }
}

/// An option's name, as this package spells it.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OptionName {
    From,
    To,
    FileName,
    Package,
    Units,
    Up,
    FromUnits,
    FromUp,
    FbxColorSpace,
    FbxLightUnits,
    UsdLightUnits,
    DaeMorphs,
    UsdzRoot,
    UsdClips,
    Fps,
}

impl From<core::OptionName> for OptionName {
    fn from(o: core::OptionName) -> OptionName {
        match o {
            core::OptionName::From => OptionName::From,
            core::OptionName::To => OptionName::To,
            core::OptionName::FileName => OptionName::FileName,
            core::OptionName::Package => OptionName::Package,
            core::OptionName::Units => OptionName::Units,
            core::OptionName::Up => OptionName::Up,
            core::OptionName::FromUnits => OptionName::FromUnits,
            core::OptionName::FromUp => OptionName::FromUp,
            core::OptionName::FbxColorSpace => OptionName::FbxColorSpace,
            core::OptionName::FbxLightUnits => OptionName::FbxLightUnits,
            core::OptionName::UsdLightUnits => OptionName::UsdLightUnits,
            core::OptionName::DaeMorphs => OptionName::DaeMorphs,
            core::OptionName::UsdzRoot => OptionName::UsdzRoot,
            core::OptionName::UsdClips => OptionName::UsdClips,
            core::OptionName::Fps => OptionName::Fps,
        }
    }
}

/// [`OptionName`]'s TypeScript spelling, for messages and `ConvertError.param`.
pub fn ts_name(o: core::OptionName) -> &'static str {
    match OptionName::from(o) {
        OptionName::From => "from",
        OptionName::To => "to",
        OptionName::FileName => "fileName",
        OptionName::Package => "package",
        OptionName::Units => "units",
        OptionName::Up => "up",
        OptionName::FromUnits => "fromUnits",
        OptionName::FromUp => "fromUp",
        OptionName::FbxColorSpace => "fbxColorSpace",
        OptionName::FbxLightUnits => "fbxLightUnits",
        OptionName::UsdLightUnits => "usdLightUnits",
        OptionName::DaeMorphs => "daeMorphs",
        OptionName::UsdzRoot => "usdzRoot",
        OptionName::UsdClips => "usdClips",
        OptionName::Fps => "fps",
    }
}

/// Why a call failed, as litegen's HTTP API spells it. `param_required` is a
/// missing required option (`to`), which only JavaScript callers can hit.
// Exists for the TypeScript union tsify declares; at runtime the codes are
// the core's `ErrorCode::as_str` (and `param_required`), which the test below
// pins equal to these spellings.
#[allow(dead_code)]
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No format was given and none could be recognised; also a set of files
    /// with no model among them.
    FormatUnrecognized,
    /// The bytes are not the format they were said to be.
    FormatMismatch,
    /// Several of the given files could be the model; pass `fileName`.
    InputAmbiguous,
    /// The input is over `maxInputBytes()`.
    InputTooLarge,
    /// The file is that format, but unreadable.
    MeshMalformed,
    /// The file uses something the converter does not model, or is past one
    /// of its budgets.
    MeshUnsupported,
    /// The model holds no triangles, lines or points.
    MeshEmpty,
    /// An option that does not apply to this format, or does not exist.
    ParamUnsupported,
    /// An option's value is not one it takes.
    ParamInvalid,
    /// A required option is missing.
    ParamRequired,
}

impl From<core::ErrorCode> for ErrorCode {
    fn from(c: core::ErrorCode) -> ErrorCode {
        match c {
            core::ErrorCode::FormatUnrecognized => ErrorCode::FormatUnrecognized,
            core::ErrorCode::FormatMismatch => ErrorCode::FormatMismatch,
            core::ErrorCode::InputAmbiguous => ErrorCode::InputAmbiguous,
            core::ErrorCode::InputTooLarge => ErrorCode::InputTooLarge,
            core::ErrorCode::MeshMalformed => ErrorCode::MeshMalformed,
            core::ErrorCode::MeshUnsupported => ErrorCode::MeshUnsupported,
            core::ErrorCode::MeshEmpty => ErrorCode::MeshEmpty,
            core::ErrorCode::ParamUnsupported => ErrorCode::ParamUnsupported,
            core::ErrorCode::ParamInvalid => ErrorCode::ParamInvalid,
        }
    }
}

/// How to read the input.
#[derive(Tsify, Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadOptions {
    /// The input's format. Detected from its bytes and file name when absent.
    #[tsify(optional)]
    pub from: Option<Format>,
    /// The input's file name: a hint for detection when the input is one
    /// file, or which of several files is the model.
    #[tsify(optional)]
    pub file_name: Option<String>,
    /// Only when `from` is `stl`, `obj` or `ply`: the unit the file's numbers
    /// are in. `m` when absent.
    #[tsify(optional)]
    pub from_units: Option<Unit>,
    /// Only when `from` is `stl`, `obj` or `ply`: the file's up axis. `y` when
    /// absent.
    #[tsify(optional)]
    pub from_up: Option<UpAxis>,
    /// Only when `from` is `fbx`: how its material colours are stored.
    #[tsify(optional)]
    pub fbx_color_space: Option<FbxColorSpace>,
    /// Only when `from` is `fbx`: how its light intensities are stored.
    /// `photometric` when absent.
    #[tsify(optional)]
    pub fbx_light_units: Option<LightUnits>,
    /// Only when `from` is `usdz`, `usda` or `usdc`: how its light
    /// intensities are stored. `photometric` when absent.
    #[tsify(optional)]
    pub usd_light_units: Option<LightUnits>,
}

impl From<ReadOptions> for core::ReadOptions {
    fn from(o: ReadOptions) -> core::ReadOptions {
        let ReadOptions { from, file_name, from_units, from_up, fbx_color_space, fbx_light_units, usd_light_units } = o;
        core::ReadOptions {
            from: from.map(Into::into),
            file_name,
            from_units: from_units.map(Into::into),
            from_up: from_up.map(Into::into),
            fbx_color_space: fbx_color_space.map(Into::into),
            fbx_light_units: fbx_light_units.map(Into::into),
            usd_light_units: usd_light_units.map(Into::into),
        }
    }
}

/// How to write the output.
#[derive(Tsify, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteOptions {
    /// The format to write.
    pub to: Format,
    /// `file` (the default), `zip` or `files`.
    #[tsify(optional)]
    pub package: Option<Packaging>,
    /// Only when `to` is `stl`, `obj` or `ply`: the unit the written numbers
    /// are in. `m` when absent.
    #[tsify(optional)]
    pub units: Option<Unit>,
    /// Only when `to` is `stl`, `obj` or `ply`: the written up axis. `y` when
    /// absent.
    #[tsify(optional)]
    pub up: Option<UpAxis>,
    /// Only when `to` is `fbx`: how material colours are written. `srgb`
    /// when absent.
    #[tsify(optional)]
    pub fbx_color_space: Option<FbxColorSpace>,
    /// Only when `to` is `fbx`: how light intensities are written.
    /// `photometric` when absent.
    #[tsify(optional)]
    pub fbx_light_units: Option<LightUnits>,
    /// Only when `to` is `usdz`, `usda` or `usdc`: how light intensities are
    /// written. `photometric` when absent.
    #[tsify(optional)]
    pub usd_light_units: Option<LightUnits>,
    /// Only when `to` is `dae`: how morph targets are written. `bake` when
    /// absent.
    #[tsify(optional)]
    pub dae_morphs: Option<DaeMorphs>,
    /// Only when `to` is `usdz`: what the package's root layer is. `usda` when
    /// absent.
    #[tsify(optional)]
    pub usdz_root: Option<UsdzRoot>,
    /// Only when `to` is `usda`, `usdc` or `usdz`: which animation clips the
    /// file holds. `all` for `usda` and `usdc` and `first` for `usdz` when
    /// absent.
    #[tsify(optional)]
    pub usd_clips: Option<UsdClips>,
    /// Only when `to` is `fbx`, `dae`, `usdz`, `usda` or `usdc`, which key
    /// animation at a fixed rate: that rate, in frames per second. 30 when
    /// absent.
    #[tsify(optional)]
    pub fps: Option<u32>,
}

impl From<WriteOptions> for core::WriteOptions {
    fn from(o: WriteOptions) -> core::WriteOptions {
        let WriteOptions {
            to,
            package,
            units,
            up,
            fbx_color_space,
            fbx_light_units,
            usd_light_units,
            dae_morphs,
            usdz_root,
            usd_clips,
            fps,
        } = o;
        core::WriteOptions {
            to: to.into(),
            package: package.map_or(core::Packaging::File, Into::into),
            units: units.map(Into::into),
            up: up.map(Into::into),
            fbx_color_space: fbx_color_space.map(Into::into),
            fbx_light_units: fbx_light_units.map(Into::into),
            usd_light_units: usd_light_units.map(Into::into),
            dae_morphs: dae_morphs.map(Into::into),
            usdz_root: usdz_root.map(Into::into),
            usd_clips: usd_clips.map(Into::into),
            fps,
        }
    }
}

/// How to read the input and write the output. `fbxColorSpace` and
/// `fbxLightUnits` are one choice each for whichever end is FBX, and
/// `usdLightUnits` for whichever end is USD.
#[derive(Tsify, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConvertOptions {
    /// The format to write.
    pub to: Format,
    /// The input's format. Detected from its bytes and file name when absent.
    #[tsify(optional)]
    pub from: Option<Format>,
    /// The input's file name: a hint for detection when the input is one
    /// file, or which of several files is the model.
    #[tsify(optional)]
    pub file_name: Option<String>,
    /// `file` (the default), `zip` or `files`.
    #[tsify(optional)]
    pub package: Option<Packaging>,
    /// Only when `to` is `stl`, `obj` or `ply`: the unit the written numbers
    /// are in. `m` when absent.
    #[tsify(optional)]
    pub units: Option<Unit>,
    /// Only when `to` is `stl`, `obj` or `ply`: the written up axis. `y` when
    /// absent.
    #[tsify(optional)]
    pub up: Option<UpAxis>,
    /// Only when `from` is `stl`, `obj` or `ply`: the unit the input's numbers
    /// are in. `m` when absent.
    #[tsify(optional)]
    pub from_units: Option<Unit>,
    /// Only when `from` is `stl`, `obj` or `ply`: the input's up axis. `y`
    /// when absent.
    #[tsify(optional)]
    pub from_up: Option<UpAxis>,
    /// Only when `from` or `to` is `fbx`: how FBX material colours are
    /// stored, at that end.
    #[tsify(optional)]
    pub fbx_color_space: Option<FbxColorSpace>,
    /// Only when `from` or `to` is `fbx`: how FBX light intensities are
    /// stored, at that end. `photometric` when absent.
    #[tsify(optional)]
    pub fbx_light_units: Option<LightUnits>,
    /// Only when `from` or `to` is `usdz`, `usda` or `usdc`: how USD light
    /// intensities are stored, at that end. `photometric` when absent.
    #[tsify(optional)]
    pub usd_light_units: Option<LightUnits>,
    /// Only when `to` is `dae`: how morph targets are written. `bake` when
    /// absent.
    #[tsify(optional)]
    pub dae_morphs: Option<DaeMorphs>,
    /// Only when `to` is `usdz`: what the package's root layer is. `usda` when
    /// absent.
    #[tsify(optional)]
    pub usdz_root: Option<UsdzRoot>,
    /// Only when `to` is `usda`, `usdc` or `usdz`: which animation clips the
    /// file holds. `all` for `usda` and `usdc` and `first` for `usdz` when
    /// absent.
    #[tsify(optional)]
    pub usd_clips: Option<UsdClips>,
    /// Only when `to` is `fbx`, `dae`, `usdz`, `usda` or `usdc`: the rate, in
    /// frames per second, animation is keyed at. 30 when absent.
    #[tsify(optional)]
    pub fps: Option<u32>,
}

impl ConvertOptions {
    /// The read half and the write half, the options of either end on both.
    pub fn split(self) -> (core::ReadOptions, core::WriteOptions) {
        let ConvertOptions {
            to,
            from,
            file_name,
            package,
            units,
            up,
            from_units,
            from_up,
            fbx_color_space,
            fbx_light_units,
            usd_light_units,
            dae_morphs,
            usdz_root,
            usd_clips,
            fps,
        } = self;
        let read = ReadOptions { from, file_name, from_units, from_up, fbx_color_space, fbx_light_units, usd_light_units };
        let write =
            WriteOptions { to, package, units, up, fbx_color_space, fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips, fps };
        (read.into(), write.into())
    }
}

/// What a file is to its model.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileRole {
    /// The model file itself.
    Model,
    /// An OBJ's `.mtl`.
    MaterialLibrary,
    /// A glTF's `.bin`, or another binary the model references.
    Buffer,
    /// An image a material uses.
    Texture,
    /// A USD layer the root layer composes.
    Layer,
    /// Given, but nothing referenced it.
    Unused,
}

impl From<core::FileRole> for FileRole {
    fn from(r: core::FileRole) -> FileRole {
        match r {
            core::FileRole::Model => FileRole::Model,
            core::FileRole::MaterialLibrary => FileRole::MaterialLibrary,
            core::FileRole::Buffer => FileRole::Buffer,
            core::FileRole::Texture => FileRole::Texture,
            core::FileRole::Layer => FileRole::Layer,
            core::FileRole::Unused => FileRole::Unused,
        }
    }
}

/// The kind of content a loss is about, spelled as litegen's HTTP API
/// spells it in `X-Litegen-Conversion-Losses`.
#[derive(Tsify, Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LossKind {
    Animation,
    Skins,
    MorphTargets,
    Normals,
    Uvs,
    VertexColors,
    Textures,
    ImageFormats,
    Materials,
    Alpha,
    Hierarchy,
    Meshes,
    TextureSampling,
    TextureTransforms,
    Geometry,
    Lights,
    Cameras,
    Points,
    Lines,
}

impl From<core::LossKind> for LossKind {
    fn from(k: core::LossKind) -> LossKind {
        match k {
            core::LossKind::Animation => LossKind::Animation,
            core::LossKind::Skins => LossKind::Skins,
            core::LossKind::MorphTargets => LossKind::MorphTargets,
            core::LossKind::Normals => LossKind::Normals,
            core::LossKind::Uvs => LossKind::Uvs,
            core::LossKind::VertexColors => LossKind::VertexColors,
            core::LossKind::Textures => LossKind::Textures,
            core::LossKind::ImageFormats => LossKind::ImageFormats,
            core::LossKind::Materials => LossKind::Materials,
            core::LossKind::Alpha => LossKind::Alpha,
            core::LossKind::Hierarchy => LossKind::Hierarchy,
            core::LossKind::Meshes => LossKind::Meshes,
            core::LossKind::TextureSampling => LossKind::TextureSampling,
            core::LossKind::TextureTransforms => LossKind::TextureTransforms,
            core::LossKind::Geometry => LossKind::Geometry,
            core::LossKind::Lights => LossKind::Lights,
            core::LossKind::Cameras => LossKind::Cameras,
            core::LossKind::Points => LossKind::Points,
            core::LossKind::Lines => LossKind::Lines,
        }
    }
}

/// One thing the target could not hold.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct Loss {
    /// What kind of thing it was.
    pub kind: LossKind,
    /// What was dropped or approximated, e.g. `animation "walk"`.
    pub what: String,
    /// Why the target could not hold it.
    pub why: String,
}

/// One written file.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OutputFile {
    /// Its name: `model.<ext>`, `model.<ext>.zip`, or a sidecar's path
    /// (`model.mtl`, `textures/…`), as the model's references name it.
    pub name: String,
    /// Its media type: the target's for the model, the file's own for a
    /// sidecar.
    pub media_type: String,
    /// What it is to the model: the model itself, a material library, a
    /// buffer or a texture.
    pub role: FileRole,
    /// Its bytes.
    #[serde(with = "serde_bytes")]
    #[tsify(type = "Uint8Array<ArrayBuffer>")]
    pub data: Vec<u8>,
}

/// A written model: its files (the model first), and everything the target
/// could not hold.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct Conversion {
    /// The format written.
    pub format: Format,
    /// The written files, the model first: one with `package` `file` or
    /// `zip`, the model and its sidecars with `files`.
    pub files: Vec<OutputFile>,
    /// What the target could not hold, judged from what the input contains.
    pub losses: Vec<Loss>,
    /// What reading the input dropped or approximated, in prose.
    pub warnings: Vec<String>,
}

impl From<core::Conversion> for Conversion {
    fn from(c: core::Conversion) -> Conversion {
        let core::Conversion { format, files, losses, warnings } = c;
        Conversion {
            format: format.into(),
            files: files
                .into_iter()
                .map(|core::OutputFile { name, media_type, role, data }| OutputFile { name, media_type: media_type.to_string(), role: role.into(), data })
                .collect(),
            losses: losses.into_iter().map(|core::Loss { kind, what, why }| Loss { kind: kind.into(), what, why }).collect(),
            warnings,
        }
    }
}

/// One input file as the read used it.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct InputFileInfo {
    /// Its name, as given (an unnamed file is `model.<ext>`).
    pub name: String,
    /// What it was to the model: the model, a file the model referenced, or
    /// `unused`.
    pub role: FileRole,
    /// Bytes.
    pub size: u32,
}

impl From<&core::InputFileInfo> for InputFileInfo {
    fn from(f: &core::InputFileInfo) -> InputFileInfo {
        let core::InputFileInfo { name, role, size } = f;
        InputFileInfo { name: name.clone(), role: (*role).into(), size: u32::try_from(*size).unwrap_or(u32::MAX) }
    }
}

/// An axis-aligned box, in metres.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct Bounds {
    /// The smallest x, y and z.
    pub min: [f64; 3],
    /// The largest x, y and z.
    pub max: [f64; 3],
}

mirror! {
    /// A material texture map: glTF's five core maps, and the layer maps of
    /// its PBR extensions.
    TextureMap from TextureMap {
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

/// One material.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct MaterialInfo {
    /// Its name, when the file gives one.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The texture maps it uses, in slot order.
    pub maps: Vec<TextureMap>,
}

mirror! {
    /// A humanoid joint naming scheme litegen knows.
    HumanoidNaming from HumanoidNaming {
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

mirror! {
    /// A VRM 1.0 humanoid bone (`VRMC_vrm-1.0/humanoid.md`), optional bones
    /// included.
    VrmBone from VrmBone {
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

/// One humanoid bone and the joint that is it.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct HumanoidBone {
    /// The bone, as VRM 1.0 names it.
    pub bone: VrmBone,
    /// The joint's name in the file.
    pub joint: String,
}

/// A humanoid rig's bones.
#[derive(Tsify, Serialize, Debug, Clone)]
pub struct Humanoid {
    /// The naming scheme its joints follow.
    pub naming: HumanoidNaming,
    /// Each VRM 1.0 humanoid bone the rig has, in VRM's order, and the joint
    /// that is it.
    pub bones: Vec<HumanoidBone>,
}

/// A skinned humanoid's rest pose.
#[derive(Tsify, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum BindPose {
    /// Arms within 15° of horizontal.
    T,
    /// Arms 25°–55° below horizontal.
    A,
    /// Neither, or no humanoid arms to measure.
    Unknown,
}

/// A model's skin: the joints that deform its mesh.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SkinInfo {
    /// How many joints it has.
    pub joints: u32,
    /// The joint every other joint descends from.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    /// Its rest pose.
    pub bind_pose: BindPose,
    /// Which joint is which humanoid bone, when the joints' names follow a
    /// naming scheme litegen knows.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub humanoid: Option<Humanoid>,
}

/// One animation clip.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnimationInfo {
    /// Its name, when the file gives one.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How long it lasts, in seconds, from 0 to its last key.
    pub duration: f64,
    /// How far the root moves, first key to last, in metres.
    pub root_travel: [f64; 3],
    /// Whether the root stays where it starts, horizontally, all clip long.
    pub in_place: bool,
}

/// What a read model holds, measured by the converter's own reader.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
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
    /// model), every instance placed. Absent for a model with no positions.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,
    /// Every material, in the file's order.
    pub materials: Vec<MaterialInfo>,
    /// The first skin, when the model has one.
    #[tsify(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinInfo>,
    /// Every animation clip.
    pub animations: Vec<AnimationInfo>,
}

impl From<&core::ModelInfo> for ModelInfo {
    fn from(i: &core::ModelInfo) -> ModelInfo {
        let core::ModelInfo { triangles, vertices, nodes, meshes, textures, uv_sets, vertex_colors, bounds, materials, skin, animations } = i;
        ModelInfo {
            triangles: *triangles,
            vertices: *vertices,
            nodes: *nodes,
            meshes: *meshes,
            textures: *textures,
            uv_sets: *uv_sets,
            vertex_colors: *vertex_colors,
            bounds: bounds.map(|core::Bounds { min, max }| Bounds { min, max }),
            materials: materials.iter().map(|m| MaterialInfo { name: m.name.clone(), maps: m.maps.iter().map(|&t| t.into()).collect() }).collect(),
            skin: skin.as_ref().map(|core::SkinInfo { joints, root, bind_pose, humanoid }| SkinInfo {
                joints: *joints,
                root: root.clone(),
                bind_pose: (*bind_pose).into(),
                humanoid: humanoid.as_ref().map(|core::Humanoid { naming, bones }| Humanoid {
                    naming: (*naming).into(),
                    bones: bones.iter().map(|core::HumanoidBone { bone, joint }| HumanoidBone { bone: (*bone).into(), joint: joint.clone() }).collect(),
                }),
            }),
            animations: animations
                .iter()
                .map(|core::AnimationInfo { name, duration_s, root_travel_m, in_place }| AnimationInfo {
                    name: name.clone(),
                    duration: *duration_s,
                    root_travel: *root_travel_m,
                    in_place: *in_place,
                })
                .collect(),
        }
    }
}

/// The frame a file's numbers are in, for a viewer whose loader does not
/// convert it: `y_up` (glTF), `z_up` (3MF), `declared` (the file says:
/// Collada, FBX, USD) or `caller` (STL, OBJ, PLY: your `up` option says).
#[derive(Tsify, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum StoredFrame {
    /// Y up, as glTF stores it.
    YUp,
    /// Z up, as 3MF stores it.
    ZUp,
    /// The frame the file declares (Collada, FBX, USD).
    Declared,
    /// The frame your `up` option names (STL, OBJ, PLY).
    Caller,
}

/// How much of a material model a format keeps.
#[derive(Tsify, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum MaterialSupport {
    /// No materials (STL).
    None,
    /// A material's base colour baked into its vertices.
    VertexColor,
    /// Flat colours and colour textures (3MF).
    ColorOnly,
    /// Diffuse, specular and emissive with textures (OBJ+MTL, FBX, Collada).
    Phong,
    /// Metallic-roughness PBR with textures (glTF, USD).
    Pbr,
}

/// A material layer (glTF's KHR_materials_* extensions).
#[derive(Tsify, Serialize, Debug, Clone, Copy)]
#[serde(rename_all = "snake_case")]
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

/// What a format keeps: one row of the converter's lossiness table.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Keeps {
    /// The node hierarchy and its transforms.
    pub hierarchy: bool,
    /// Vertex normals.
    pub normals: bool,
    /// How many uv sets it keeps per mesh.
    pub uv_sets: u32,
    /// Vertex colours.
    pub vertex_colors: bool,
    /// How much of a material it keeps.
    pub materials: MaterialSupport,
    /// Texture images.
    pub textures: bool,
    /// Skins: joints and weights.
    pub skins: bool,
    /// Morph targets (blend shapes).
    pub morph_targets: bool,
    /// Animation clips.
    pub animation: bool,
    /// Several meshes, kept apart.
    pub multiple_meshes: bool,
    /// Texture filtering (nearest or linear).
    pub texture_filters: bool,
    /// Texture wrap modes.
    pub texture_wrap: bool,
    /// Texture offset, scale and rotation (KHR_texture_transform).
    pub texture_transforms: bool,
    /// The material layers it keeps.
    pub layers: Vec<MaterialLayer>,
    /// Lights (directional, point and spot).
    pub lights: bool,
    /// Cameras (perspective and orthographic).
    pub cameras: bool,
    /// Point primitives (otherwise left out).
    pub points: bool,
    /// Line primitives (otherwise left out).
    pub lines: bool,
}

impl From<core::Keeps> for Keeps {
    fn from(k: core::Keeps) -> Keeps {
        let core::Keeps {
            hierarchy,
            normals,
            uv_sets,
            vertex_colors,
            materials,
            textures,
            skins,
            morph_targets,
            animation,
            multiple_meshes,
            texture_filters,
            texture_wrap,
            texture_transforms,
            layers,
            lights,
            cameras,
            points,
            lines,
        } = k;
        Keeps {
            hierarchy,
            normals,
            uv_sets,
            vertex_colors,
            materials: materials.into(),
            textures,
            skins,
            morph_targets,
            animation,
            multiple_meshes,
            texture_filters,
            texture_wrap,
            texture_transforms,
            layers: layers.into_iter().map(MaterialLayer::from).collect(),
            lights,
            cameras,
            points,
            lines,
        }
    }
}

/// Everything about a format a caller needs.
#[derive(Tsify, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FormatInfo {
    /// The format.
    pub format: Format,
    /// Its human name, e.g. `glTF Binary`.
    pub label: String,
    /// The extensions it reads from, lowercase, no dot.
    pub extensions: Vec<String>,
    /// The media type its model file is sent with.
    pub media_type: String,
    /// Whether, written as a package, it has files beside the model.
    pub writes_sidecars: bool,
    /// The frame its numbers are stored in, for a viewer whose loader does
    /// not convert it.
    pub stored_frame: StoredFrame,
    /// What it keeps as one file.
    pub keeps: Keeps,
    /// What it keeps as a package (`zip` or `files`).
    pub keeps_in_package: Keeps,
    /// The read options it takes, beyond `from` and `fileName`.
    pub read_options: Vec<OptionName>,
    /// The write options it takes, beyond `to` and `package`.
    pub write_options: Vec<OptionName>,
}

impl From<core::FormatInfo> for FormatInfo {
    fn from(i: core::FormatInfo) -> FormatInfo {
        let core::FormatInfo {
            format,
            label,
            extensions,
            media_type,
            writes_sidecars,
            stored_frame,
            keeps,
            keeps_in_package,
            read_options,
            write_options,
        } = i;
        FormatInfo {
            format: format.into(),
            label: label.to_string(),
            extensions: extensions.into_iter().map(String::from).collect(),
            media_type: media_type.to_string(),
            writes_sidecars,
            stored_frame: stored_frame.into(),
            keeps: keeps.into(),
            keeps_in_package: keeps_in_package.into(),
            read_options: read_options.into_iter().map(Into::into).collect(),
            write_options: write_options.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::BindPose> for BindPose {
    fn from(p: core::BindPose) -> BindPose {
        match p {
            core::BindPose::T => BindPose::T,
            core::BindPose::A => BindPose::A,
            core::BindPose::Unknown => BindPose::Unknown,
        }
    }
}

impl From<core::StoredFrame> for StoredFrame {
    fn from(f: core::StoredFrame) -> StoredFrame {
        match f {
            core::StoredFrame::YUp => StoredFrame::YUp,
            core::StoredFrame::ZUp => StoredFrame::ZUp,
            core::StoredFrame::Declared => StoredFrame::Declared,
            core::StoredFrame::Caller => StoredFrame::Caller,
        }
    }
}

impl From<core::MaterialSupport> for MaterialSupport {
    fn from(m: core::MaterialSupport) -> MaterialSupport {
        match m {
            core::MaterialSupport::None => MaterialSupport::None,
            core::MaterialSupport::VertexColor => MaterialSupport::VertexColor,
            core::MaterialSupport::ColorOnly => MaterialSupport::ColorOnly,
            core::MaterialSupport::Phong => MaterialSupport::Phong,
            core::MaterialSupport::Pbr => MaterialSupport::Pbr,
        }
    }
}

impl From<core::MaterialLayer> for MaterialLayer {
    fn from(l: core::MaterialLayer) -> MaterialLayer {
        match l {
            core::MaterialLayer::Clearcoat => MaterialLayer::Clearcoat,
            core::MaterialLayer::Transmission => MaterialLayer::Transmission,
            core::MaterialLayer::Volume => MaterialLayer::Volume,
            core::MaterialLayer::Ior => MaterialLayer::Ior,
            core::MaterialLayer::Specular => MaterialLayer::Specular,
            core::MaterialLayer::Sheen => MaterialLayer::Sheen,
            core::MaterialLayer::Iridescence => MaterialLayer::Iridescence,
            core::MaterialLayer::Anisotropy => MaterialLayer::Anisotropy,
            core::MaterialLayer::Dispersion => MaterialLayer::Dispersion,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spelled<T: Serialize>(v: T) -> String {
        serde_json::to_value(v).unwrap().as_str().unwrap().to_string()
    }

    #[test]
    fn closed_sets_spell_like_the_core() {
        for f in core::Format::ALL {
            assert_eq!(spelled(Format::from(f)), f.as_str());
        }
        for t in core::TextureMap::ALL {
            assert_eq!(spelled(TextureMap::from(t)), t.as_str());
        }
        for n in core::HumanoidNaming::ALL {
            assert_eq!(spelled(HumanoidNaming::from(n)), n.as_str());
        }
        for b in core::VrmBone::ALL {
            assert_eq!(spelled(VrmBone::from(b)), b.as_str());
        }
        for o in core::OptionName::ALL {
            assert_eq!(spelled(OptionName::from(o)), ts_name(o));
        }
        for c in core::ErrorCode::ALL {
            assert_eq!(spelled(ErrorCode::from(c)), c.as_str());
        }
        for r in core::FileRole::ALL {
            assert_eq!(spelled(FileRole::from(r)), r.as_str());
        }
        for k in core::LossKind::ALL {
            assert_eq!(spelled(LossKind::from(k)), k.as_str());
        }
        for f in core::StoredFrame::ALL {
            assert_eq!(spelled(StoredFrame::from(f)), f.as_str());
        }
        for m in core::MaterialSupport::ALL {
            assert_eq!(spelled(MaterialSupport::from(m)), m.as_str());
        }
        for l in core::MaterialLayer::ALL {
            assert_eq!(spelled(MaterialLayer::from(l)), l.as_str());
        }
        for p in core::BindPose::ALL {
            assert_eq!(spelled(BindPose::from(p)), p.as_str());
        }
    }

    /// The sets that only come in (options): the core's spelling reads back
    /// as the same value.
    #[test]
    fn option_values_read_as_the_core_spells_them() {
        fn read<T: serde::de::DeserializeOwned>(s: &str) -> T {
            serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or_else(|e| panic!("{s:?}: {e}"))
        }
        for u in core::Unit::ALL {
            assert_eq!(core::Unit::from(read::<Unit>(u.as_str())), u);
        }
        for a in core::UpAxis::ALL {
            assert_eq!(core::UpAxis::from(read::<UpAxis>(a.as_str())), a);
        }
        for c in core::FbxColorSpace::ALL {
            assert_eq!(core::FbxColorSpace::from(read::<FbxColorSpace>(c.as_str())), c);
        }
        for u in core::LightUnits::ALL {
            assert_eq!(core::LightUnits::from(read::<LightUnits>(u.as_str())), u);
        }
        for m in core::DaeMorphs::ALL {
            assert_eq!(core::DaeMorphs::from(read::<DaeMorphs>(m.as_str())), m);
        }
        for r in core::UsdzRoot::ALL {
            assert_eq!(core::UsdzRoot::from(read::<UsdzRoot>(r.as_str())), r);
        }
        for c in core::UsdClips::ALL {
            assert_eq!(core::UsdClips::from(read::<UsdClips>(c.as_str())), c);
        }
        for p in core::Packaging::ALL {
            assert_eq!(core::Packaging::from(read::<Packaging>(p.as_str())), p);
        }
        for f in core::Format::ALL {
            assert_eq!(core::Format::from(read::<Format>(f.as_str())), f);
        }
    }
}
