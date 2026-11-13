//! The eleven containers and what each one keeps, from mesh's own capability
//! table, so a binding or an app never encodes a format fact itself.

use litegen_mesh::MeshFormat;

use crate::OptionName;

/// A container the converter reads and writes. Every pair converts, both
/// ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    /// glTF Binary.
    Glb,
    /// glTF as JSON: buffers embedded as data URIs as one file, or a `.bin`
    /// and images beside it as a package.
    Gltf,
    /// Wavefront OBJ: materials and textures through an MTL, as a package.
    Obj,
    /// STL: one mesh of triangles.
    Stl,
    /// PLY: one mesh, with normals, one uv set and vertex colour.
    Ply,
    /// 3D Manufacturing Format: a zip, textures inside, +Z up.
    ThreeMf,
    /// COLLADA 1.4.1: textures as sidecars, as a package.
    Dae,
    /// Autodesk FBX: binary 7100–7700 and ASCII in, binary 7400 out.
    Fbx,
    /// USD packaged as a zip: one file, textures inside.
    Usdz,
    /// A standalone USD layer written as USDA text. Reads either encoding,
    /// text or a binary crate.
    Usda,
    /// A standalone USD layer written as a binary crate. Reads either
    /// encoding, as [`Format::Usda`] does.
    Usdc,
}

impl Format {
    /// Every format, in the converter's stable order.
    pub const ALL: [Format; 11] = [
        Format::Glb,
        Format::Gltf,
        Format::Obj,
        Format::Stl,
        Format::Ply,
        Format::ThreeMf,
        Format::Dae,
        Format::Fbx,
        Format::Usdz,
        Format::Usda,
        Format::Usdc,
    ];

    /// The lowercase extension, without a dot: litegen's spelling.
    pub fn as_str(self) -> &'static str {
        MeshFormat::from(self).as_str()
    }

    /// A format name, case-insensitive. `usd` is not one (it names either
    /// encoding), so it gives `None`.
    pub fn parse(s: &str) -> Option<Format> {
        MeshFormat::parse(s).map(Format::from)
    }

    /// The media type litegen serves the format as.
    pub fn media_type(self) -> &'static str {
        match self {
            Format::Glb => "model/gltf-binary",
            Format::Gltf => "model/gltf+json",
            Format::Obj => "model/obj",
            Format::Stl => "model/stl",
            Format::Ply => "application/octet-stream",
            Format::ThreeMf => "model/3mf",
            Format::Dae => "model/vnd.collada+xml",
            Format::Fbx => "application/octet-stream",
            Format::Usdz => "model/vnd.usdz+zip",
            Format::Usda => "model/vnd.usda",
            // IANA registers no type for a binary USD crate.
            Format::Usdc => "application/octet-stream",
        }
    }

    /// A short human name.
    pub fn label(self) -> &'static str {
        match self {
            Format::Glb => "glTF Binary",
            Format::Gltf => "glTF",
            Format::Obj => "Wavefront OBJ",
            Format::Stl => "STL",
            Format::Ply => "PLY",
            Format::ThreeMf => "3MF",
            Format::Dae => "COLLADA",
            Format::Fbx => "FBX",
            Format::Usdz => "USDZ",
            Format::Usda => "USD (text)",
            Format::Usdc => "USD (binary)",
        }
    }

    /// Everything a caller needs to know about the format.
    pub fn info(self) -> FormatInfo {
        let mesh = MeshFormat::from(self);
        // Every field named, so that a new mesh option breaks this line and
        // gets decided here (and in the options), not defaulted past.
        let file = litegen_mesh::WriteOptions {
            self_contained: true,
            fps: None,
            fbx_color_space: litegen_mesh::FbxColorSpace::Srgb,
            fbx_light_units: litegen_mesh::LightUnits::Photometric,
            usd_light_units: litegen_mesh::LightUnits::Photometric,
            dae_morphs: litegen_mesh::DaeMorphs::Bake,
            usdz_root: litegen_mesh::UsdzRoot::Usda,
            usd_clips: None,
        };
        let package = litegen_mesh::WriteOptions { self_contained: false, ..file };
        FormatInfo {
            format: self,
            label: self.label(),
            // `.usd` names either encoding, so both list it; the bytes decide.
            extensions: match self {
                Format::Usda | Format::Usdc => vec![self.as_str(), "usd"],
                other => vec![other.as_str()],
            },
            media_type: self.media_type(),
            writes_sidecars: mesh.writes_sidecars(),
            stored_frame: match self {
                Format::Glb | Format::Gltf => StoredFrame::YUp,
                Format::ThreeMf => StoredFrame::ZUp,
                Format::Dae | Format::Fbx | Format::Usdz | Format::Usda | Format::Usdc => StoredFrame::Declared,
                Format::Stl | Format::Obj | Format::Ply => StoredFrame::Caller,
            },
            keeps: keeps(mesh.capabilities(file)),
            keeps_in_package: keeps(mesh.capabilities(package)),
            read_options: match self {
                Format::Stl | Format::Obj | Format::Ply => vec![OptionName::FromUnits, OptionName::FromUp],
                Format::Fbx => vec![OptionName::FbxColorSpace, OptionName::FbxLightUnits],
                Format::Usdz | Format::Usda | Format::Usdc => vec![OptionName::UsdLightUnits],
                Format::Glb | Format::Gltf | Format::ThreeMf | Format::Dae => vec![],
            },
            write_options: match self {
                Format::Stl | Format::Obj | Format::Ply => vec![OptionName::Units, OptionName::Up],
                Format::Fbx => vec![OptionName::FbxColorSpace, OptionName::FbxLightUnits, OptionName::Fps],
                Format::Usdz => vec![OptionName::UsdLightUnits, OptionName::UsdzRoot, OptionName::UsdClips, OptionName::Fps],
                Format::Usda | Format::Usdc => vec![OptionName::UsdLightUnits, OptionName::UsdClips, OptionName::Fps],
                Format::Dae => vec![OptionName::DaeMorphs, OptionName::Fps],
                Format::Glb | Format::Gltf | Format::ThreeMf => vec![],
            },
        }
    }
}

impl From<MeshFormat> for Format {
    fn from(f: MeshFormat) -> Format {
        match f {
            MeshFormat::Glb => Format::Glb,
            MeshFormat::Gltf => Format::Gltf,
            MeshFormat::Obj => Format::Obj,
            MeshFormat::Stl => Format::Stl,
            MeshFormat::Ply => Format::Ply,
            MeshFormat::ThreeMf => Format::ThreeMf,
            MeshFormat::Dae => Format::Dae,
            MeshFormat::Fbx => Format::Fbx,
            MeshFormat::Usdz => Format::Usdz,
            MeshFormat::Usda => Format::Usda,
            MeshFormat::Usdc => Format::Usdc,
        }
    }
}

impl From<Format> for MeshFormat {
    fn from(f: Format) -> MeshFormat {
        match f {
            Format::Glb => MeshFormat::Glb,
            Format::Gltf => MeshFormat::Gltf,
            Format::Obj => MeshFormat::Obj,
            Format::Stl => MeshFormat::Stl,
            Format::Ply => MeshFormat::Ply,
            Format::ThreeMf => MeshFormat::ThreeMf,
            Format::Dae => MeshFormat::Dae,
            Format::Fbx => MeshFormat::Fbx,
            Format::Usdz => MeshFormat::Usdz,
            Format::Usda => MeshFormat::Usda,
            Format::Usdc => MeshFormat::Usdc,
        }
    }
}

/// Every format's [`FormatInfo`], in [`Format::ALL`] order.
pub fn formats() -> Vec<FormatInfo> {
    Format::ALL.into_iter().map(Format::info).collect()
}

impl StoredFrame {
    pub const ALL: [StoredFrame; 4] = [StoredFrame::YUp, StoredFrame::ZUp, StoredFrame::Declared, StoredFrame::Caller];

    pub fn as_str(self) -> &'static str {
        match self {
            StoredFrame::YUp => "y_up",
            StoredFrame::ZUp => "z_up",
            StoredFrame::Declared => "declared",
            StoredFrame::Caller => "caller",
        }
    }
}

/// The frame a file's numbers are in, for a viewer that draws the file with
/// a loader that does not convert it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoredFrame {
    /// +Y up, by the format's own specification (glTF).
    YUp,
    /// +Z up, by the format's own specification (3MF Core §3.1).
    ZUp,
    /// The file names its own up axis and unit (Collada, FBX, USD).
    Declared,
    /// The format has no frame of its own: the caller's `up` and `units`
    /// (or `from_up` and `from_units`) say what it is (STL, OBJ, PLY).
    Caller,
}

impl MaterialSupport {
    pub const ALL: [MaterialSupport; 5] =
        [MaterialSupport::None, MaterialSupport::VertexColor, MaterialSupport::ColorOnly, MaterialSupport::Phong, MaterialSupport::Pbr];

    pub fn as_str(self) -> &'static str {
        match self {
            MaterialSupport::None => "none",
            MaterialSupport::VertexColor => "vertex_color",
            MaterialSupport::ColorOnly => "color_only",
            MaterialSupport::Phong => "phong",
            MaterialSupport::Pbr => "pbr",
        }
    }
}

/// How much of a material model a format keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaterialSupport {
    /// No materials (STL).
    None,
    /// A material's base colour baked into its vertices (PLY, one-file OBJ).
    VertexColor,
    /// Flat colours and colour textures (3MF).
    ColorOnly,
    /// Diffuse, specular and emissive with textures (OBJ+MTL, FBX, Collada).
    Phong,
    /// Metallic-roughness PBR with textures (glTF, USD).
    Pbr,
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
}

/// A material layer a format can keep (glTF's KHR_materials_* extensions).
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

/// What a format keeps: one row of mesh's lossiness table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keeps {
    /// Separate nodes with their own transforms (otherwise baked into world
    /// space).
    pub hierarchy: bool,
    pub normals: bool,
    /// How many uv sets survive.
    pub uv_sets: u32,
    pub vertex_colors: bool,
    pub materials: MaterialSupport,
    pub textures: bool,
    pub skins: bool,
    pub morph_targets: bool,
    pub animation: bool,
    /// More than one primitive survives as separate draws.
    pub multiple_meshes: bool,
    pub texture_filters: bool,
    pub texture_wrap: bool,
    /// Texture transforms survive as such (otherwise baked into the uvs).
    pub texture_transforms: bool,
    /// The material layers that survive.
    pub layers: Vec<MaterialLayer>,
    /// Lights survive (directional, point and spot).
    pub lights: bool,
    /// Cameras survive (perspective and orthographic).
    pub cameras: bool,
    /// Point primitives survive (otherwise left out).
    pub points: bool,
    /// Line primitives survive (otherwise left out).
    pub lines: bool,
}

/// Everything about a format a binding or an app needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatInfo {
    pub format: Format,
    pub label: &'static str,
    /// The extensions it reads from, lowercase, no dot.
    pub extensions: Vec<&'static str>,
    pub media_type: &'static str,
    /// Whether, written as a package, it has files beside the main one.
    pub writes_sidecars: bool,
    pub stored_frame: StoredFrame,
    /// What it keeps as one file.
    pub keeps: Keeps,
    /// What it keeps as a package (zip or files).
    pub keeps_in_package: Keeps,
    /// The read options it takes, beyond `from` and `file_name`.
    pub read_options: Vec<OptionName>,
    /// The write options it takes, beyond `to` and `package`.
    pub write_options: Vec<OptionName>,
}

fn keeps(c: litegen_mesh::Capabilities) -> Keeps {
    let litegen_mesh::Capabilities {
        hierarchy,
        normals,
        uv_sets,
        vertex_colors,
        materials,
        textures,
        skinning,
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
    } = c;
    let litegen_mesh::scene::LayerSupport {
        clearcoat,
        transmission,
        volume,
        ior,
        specular,
        sheen,
        iridescence,
        anisotropy,
        dispersion,
    } = layers;
    let layers = [
        (clearcoat, MaterialLayer::Clearcoat),
        (transmission, MaterialLayer::Transmission),
        (volume, MaterialLayer::Volume),
        (ior, MaterialLayer::Ior),
        (specular, MaterialLayer::Specular),
        (sheen, MaterialLayer::Sheen),
        (iridescence, MaterialLayer::Iridescence),
        (anisotropy, MaterialLayer::Anisotropy),
        (dispersion, MaterialLayer::Dispersion),
    ]
    .into_iter()
    .filter_map(|(kept, layer)| kept.then_some(layer))
    .collect();
    Keeps {
        hierarchy,
        normals,
        uv_sets: u32::try_from(uv_sets).unwrap_or(u32::MAX),
        vertex_colors,
        materials: match materials {
            litegen_mesh::MaterialSupport::None => MaterialSupport::None,
            litegen_mesh::MaterialSupport::VertexColor => MaterialSupport::VertexColor,
            litegen_mesh::MaterialSupport::ColorOnly => MaterialSupport::ColorOnly,
            litegen_mesh::MaterialSupport::Phong => MaterialSupport::Phong,
            litegen_mesh::MaterialSupport::Pbr => MaterialSupport::Pbr,
        },
        textures,
        skins: skinning,
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
    }
}
