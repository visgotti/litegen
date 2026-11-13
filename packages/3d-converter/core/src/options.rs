//! The options a read and a write take: the HTTP converter's parameters
//! (`from`, `to`, `units`, `up`, `from_units`, `from_up`, `fbx_color_space`,
//! `fbx_light_units`, `usd_light_units`, `dae_morphs`, `usdz_root`,
//! `usd_clips`, `package`; not
//! `ktx2_textures`, whose `keep` the package does not offer, so it writes as
//! the default, `png`, does), plus mesh's `fps`, which the HTTP API does not
//! expose.

use crate::Format;

/// A length unit, for the containers with none of their own (STL, OBJ, PLY).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Metres, glTF's unit.
    M,
    Cm,
    /// Millimetres: what slicers read STL as.
    Mm,
    In,
    Ft,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::M, Unit::Cm, Unit::Mm, Unit::In, Unit::Ft];

    pub fn as_str(self) -> &'static str {
        match self {
            Unit::M => "m",
            Unit::Cm => "cm",
            Unit::Mm => "mm",
            Unit::In => "in",
            Unit::Ft => "ft",
        }
    }

    /// Case-insensitive, surrounding space ignored.
    pub fn parse(s: &str) -> Option<Unit> {
        let s = s.trim();
        Unit::ALL.into_iter().find(|u| u.as_str().eq_ignore_ascii_case(s))
    }

    /// Metres per unit, as mesh's frame normalisation counts them.
    pub fn metres(self) -> f64 {
        litegen_mesh::normalize::metres_per_unit(self.as_str()).expect("mesh knows every unit")
    }
}

/// The axis a unitless file's "up" is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UpAxis {
    /// +Y up, glTF's frame.
    Y,
    /// +Z up (slicers, CAD, Blender): +Y up becomes +Z up and the front (+Z)
    /// becomes −Y.
    Z,
}

impl UpAxis {
    pub const ALL: [UpAxis; 2] = [UpAxis::Y, UpAxis::Z];

    pub fn as_str(self) -> &'static str {
        match self {
            UpAxis::Y => "y",
            UpAxis::Z => "z",
        }
    }

    pub fn parse(s: &str) -> Option<UpAxis> {
        // A signed axis too (`+z`), as litegen's HTTP API takes it.
        let s = s.trim();
        let s = s.strip_prefix('+').unwrap_or(s);
        UpAxis::ALL.into_iter().find(|a| a.as_str().eq_ignore_ascii_case(s))
    }
}

/// How an FBX file's material colours are stored. FBX does not say, and its
/// readers disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FbxColorSpace {
    /// sRGB-encoded, as three.js's FBXLoader reads them (the default).
    Srgb,
    /// Linear, as Blender, Unity, Unreal, ufbx and assimp read them.
    Linear,
}

impl FbxColorSpace {
    pub const ALL: [FbxColorSpace; 2] = [FbxColorSpace::Srgb, FbxColorSpace::Linear];

    pub fn as_str(self) -> &'static str {
        match self {
            FbxColorSpace::Srgb => "srgb",
            FbxColorSpace::Linear => "linear",
        }
    }

    pub fn parse(s: &str) -> Option<FbxColorSpace> {
        let s = s.trim();
        FbxColorSpace::ALL.into_iter().find(|c| c.as_str().eq_ignore_ascii_case(s))
    }
}

impl From<FbxColorSpace> for litegen_mesh::FbxColorSpace {
    fn from(c: FbxColorSpace) -> litegen_mesh::FbxColorSpace {
        match c {
            FbxColorSpace::Srgb => litegen_mesh::FbxColorSpace::Srgb,
            FbxColorSpace::Linear => litegen_mesh::FbxColorSpace::Linear,
        }
    }
}

/// How an FBX or a USD file's light intensities are stored. Neither format
/// fixes a unit, and the tools that read them disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LightUnits {
    /// glTF's photometric units (candela for a point or spot light, lux for a
    /// directional one), carried over as ufbx and three.js read FBX (the
    /// default).
    Photometric,
    /// Blender's watts, as Blender's glTF importer converts glTF's lights: the
    /// file opens in Blender as bright as the scene's glTF does.
    Blender,
}

impl LightUnits {
    pub const ALL: [LightUnits; 2] = [LightUnits::Photometric, LightUnits::Blender];

    pub fn as_str(self) -> &'static str {
        match self {
            LightUnits::Photometric => "photometric",
            LightUnits::Blender => "blender",
        }
    }

    pub fn parse(s: &str) -> Option<LightUnits> {
        let s = s.trim();
        LightUnits::ALL.into_iter().find(|u| u.as_str().eq_ignore_ascii_case(s))
    }
}

impl From<LightUnits> for litegen_mesh::LightUnits {
    fn from(u: LightUnits) -> litegen_mesh::LightUnits {
        match u {
            LightUnits::Photometric => litegen_mesh::LightUnits::Photometric,
            LightUnits::Blender => litegen_mesh::LightUnits::Blender,
        }
    }
}

/// How a Collada file holds morph targets (blend shapes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DaeMorphs {
    /// Each node's mesh is written at that node's weights, and the targets and
    /// their weight animation are dropped and reported as losses: the file
    /// every Collada reader opens (the default).
    /// [`crate::FormatInfo::keeps`] describes this.
    Bake,
    /// The targets are written as `<morph>` controllers, with their weight
    /// animation, so the file keeps morph targets. SceneKit (and so Xcode)
    /// and Godot read them, while three.js's ColladaLoader and assimp refuse
    /// the whole file, so choose it only for a reader that opens controllers.
    /// Two kinds of scene that `bake` writes are refused here
    /// (`mesh_unsupported`): one with a weight animation too long to key at
    /// the frame rate (`bake` drops the animation), and one whose controllers
    /// would pass the converter's 4,000,000-vertex limit (`bake` writes no
    /// controllers).
    Keep,
}

impl DaeMorphs {
    pub const ALL: [DaeMorphs; 2] = [DaeMorphs::Bake, DaeMorphs::Keep];

    pub fn as_str(self) -> &'static str {
        match self {
            DaeMorphs::Bake => "bake",
            DaeMorphs::Keep => "keep",
        }
    }

    pub fn parse(s: &str) -> Option<DaeMorphs> {
        let s = s.trim();
        DaeMorphs::ALL.into_iter().find(|m| m.as_str().eq_ignore_ascii_case(s))
    }
}

impl From<DaeMorphs> for litegen_mesh::DaeMorphs {
    fn from(m: DaeMorphs) -> litegen_mesh::DaeMorphs {
        match m {
            DaeMorphs::Bake => litegen_mesh::DaeMorphs::Bake,
            DaeMorphs::Keep => litegen_mesh::DaeMorphs::Keep,
        }
    }
}

/// What a USDZ package's root layer, its first entry, is written as. The two
/// read back as one scene, and every entry is stored and 64-byte aligned
/// either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsdzRoot {
    /// `model.usda`, USDA text (the default): `usdchecker --arkit` accepts it
    /// (the rule demanding a crate was removed in OpenUSD 24.03), and
    /// `<model-viewer>` hands it to AR Quick Look as it is.
    Usda,
    /// `model.usdc`, the binary crate, as Apple's own packagers write it.
    Usdc,
}

impl UsdzRoot {
    pub const ALL: [UsdzRoot; 2] = [UsdzRoot::Usda, UsdzRoot::Usdc];

    pub fn as_str(self) -> &'static str {
        match self {
            UsdzRoot::Usda => "usda",
            UsdzRoot::Usdc => "usdc",
        }
    }

    pub fn parse(s: &str) -> Option<UsdzRoot> {
        let s = s.trim();
        UsdzRoot::ALL.into_iter().find(|r| r.as_str().eq_ignore_ascii_case(s))
    }
}

impl From<UsdzRoot> for litegen_mesh::UsdzRoot {
    fn from(r: UsdzRoot) -> litegen_mesh::UsdzRoot {
        match r {
            UsdzRoot::Usda => litegen_mesh::UsdzRoot::Usda,
            UsdzRoot::Usdc => litegen_mesh::UsdzRoot::Usdc,
        }
    }
}

/// Which animation clips a USD file holds. USD has one timeline, so a scene
/// with several clips keeps them as alternatives or keeps the first. Absent,
/// the container's default: `all` for usda and usdc, and `first` for usdz
/// until AR Quick Look's behaviour with variants is checked on a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UsdClips {
    /// Every clip, as an alternative: a variant set `Animation` on the
    /// default prim with one variant per clip, the first selected. A reader
    /// that composes variants can switch between them; one that does not
    /// plays the first. Every clip's skeletal and blend-shape animation is
    /// kept; rigid (node) motion is the first clip's only, the other clips'
    /// is reported as losses, and a scene whose clips are all rigid writes as
    /// [`UsdClips::First`] does.
    All,
    /// The first clip alone; the others are reported as losses.
    First,
}

impl UsdClips {
    pub const ALL: [UsdClips; 2] = [UsdClips::All, UsdClips::First];

    pub fn as_str(self) -> &'static str {
        match self {
            UsdClips::All => "all",
            UsdClips::First => "first",
        }
    }

    pub fn parse(s: &str) -> Option<UsdClips> {
        let s = s.trim();
        UsdClips::ALL.into_iter().find(|c| c.as_str().eq_ignore_ascii_case(s))
    }
}

impl From<UsdClips> for litegen_mesh::UsdClips {
    fn from(c: UsdClips) -> litegen_mesh::UsdClips {
        match c {
            UsdClips::All => litegen_mesh::UsdClips::All,
            UsdClips::First => litegen_mesh::UsdClips::First,
        }
    }
}

/// What a conversion answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Packaging {
    /// One self-contained file (the default). Containers that reference
    /// sidecars embed them (glTF) or leave them out and report the loss
    /// (OBJ's materials become vertex colours; Collada and USDA lose their
    /// textures).
    File,
    /// One zip of the file and the sidecars it references, the file first.
    Zip,
    /// The file and its sidecars as separate files, the file first.
    Files,
}

impl Packaging {
    pub const ALL: [Packaging; 3] = [Packaging::File, Packaging::Zip, Packaging::Files];

    pub fn as_str(self) -> &'static str {
        match self {
            Packaging::File => "file",
            Packaging::Zip => "zip",
            Packaging::Files => "files",
        }
    }

    pub fn parse(s: &str) -> Option<Packaging> {
        let s = s.trim();
        Packaging::ALL.into_iter().find(|p| p.as_str().eq_ignore_ascii_case(s))
    }
}

/// An option of a read or a write, for errors and for [`crate::FormatInfo`]'s
/// lists of which options a format takes. Each binding spells these its own
/// way; [`OptionName::as_str`] is the Rust field name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl OptionName {
    pub const ALL: [OptionName; 15] = [
        OptionName::From,
        OptionName::To,
        OptionName::FileName,
        OptionName::Package,
        OptionName::Units,
        OptionName::Up,
        OptionName::FromUnits,
        OptionName::FromUp,
        OptionName::FbxColorSpace,
        OptionName::FbxLightUnits,
        OptionName::UsdLightUnits,
        OptionName::DaeMorphs,
        OptionName::UsdzRoot,
        OptionName::UsdClips,
        OptionName::Fps,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OptionName::From => "from",
            OptionName::To => "to",
            OptionName::FileName => "file_name",
            OptionName::Package => "package",
            OptionName::Units => "units",
            OptionName::Up => "up",
            OptionName::FromUnits => "from_units",
            OptionName::FromUp => "from_up",
            OptionName::FbxColorSpace => "fbx_color_space",
            OptionName::FbxLightUnits => "fbx_light_units",
            OptionName::UsdLightUnits => "usd_light_units",
            OptionName::DaeMorphs => "dae_morphs",
            OptionName::UsdzRoot => "usdz_root",
            OptionName::UsdClips => "usd_clips",
            OptionName::Fps => "fps",
        }
    }
}

/// How to read the input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadOptions {
    /// The input's format. Detected from its bytes and name when absent.
    pub from: Option<Format>,
    /// The input file's name, when it is one file: a hint for detection and
    /// the name its role is reported under.
    pub file_name: Option<String>,
    /// Only for stl, obj and ply, which carry no units: the unit the file's
    /// numbers are in. Metres when absent.
    pub from_units: Option<Unit>,
    /// Only for stl, obj and ply: the file's up axis. +Y when absent.
    pub from_up: Option<UpAxis>,
    /// Only for fbx: how its material colours are stored. sRGB when absent.
    pub fbx_color_space: Option<FbxColorSpace>,
    /// Only for fbx: how its light intensities are stored. Photometric when
    /// absent.
    pub fbx_light_units: Option<LightUnits>,
    /// Only for usdz, usda and usdc: how its light intensities are stored.
    /// Photometric when absent.
    pub usd_light_units: Option<LightUnits>,
}

/// How to write the output.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOptions {
    /// The format to write.
    pub to: Format,
    /// One file (the default), a zip, or separate files.
    pub package: Packaging,
    /// Only for stl, obj and ply: the unit the written numbers are in.
    /// Metres when absent.
    pub units: Option<Unit>,
    /// Only for stl, obj and ply: the written up axis. +Y when absent.
    pub up: Option<UpAxis>,
    /// Only for fbx: how material colours are written. sRGB when absent.
    pub fbx_color_space: Option<FbxColorSpace>,
    /// Only for fbx: how light intensities are written. Photometric when
    /// absent.
    pub fbx_light_units: Option<LightUnits>,
    /// Only for usdz, usda and usdc: how light intensities are written.
    /// Photometric when absent.
    pub usd_light_units: Option<LightUnits>,
    /// Only for dae: how morph targets are written. Bake when absent.
    pub dae_morphs: Option<DaeMorphs>,
    /// Only for usdz: what the package's root layer is written as. Usda when
    /// absent.
    pub usdz_root: Option<UsdzRoot>,
    /// Only for usda, usdc and usdz: which animation clips are kept. The
    /// container's default when absent ([`UsdClips`]).
    pub usd_clips: Option<UsdClips>,
    /// Only for the formats that resample animation (fbx, dae, usdz, usda,
    /// usdc): the rate, in frames per second, their keys are written at. 30
    /// when absent.
    pub fps: Option<u32>,
}

impl WriteOptions {
    /// `to` as one file, every other option at its default.
    pub fn new(to: Format) -> WriteOptions {
        WriteOptions {
            to,
            package: Packaging::File,
            units: None,
            up: None,
            fbx_color_space: None,
            fbx_light_units: None,
            usd_light_units: None,
            dae_morphs: None,
            usdz_root: None,
            usd_clips: None,
            fps: None,
        }
    }
}
