//! The closed sets and the format table every binding exposes: their
//! spellings (the HTTP API's and mesh's), and that what a format keeps and
//! which options it takes come from mesh's own capability table.

use litegen_3d_converter::{
    BindPose,
    formats, DaeMorphs, ErrorCode, FbxColorSpace, Format, Keeps, LightUnits, LossKind, MaterialLayer, MaterialSupport,
    OptionName, Packaging, StoredFrame, Unit, UpAxis, UsdClips, UsdzRoot,
};
use litegen_mesh::{MeshError, MeshFormat};

use Format::*;

#[test]
fn format_names_round_trip() {
    let names: Vec<&str> = Format::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(names, ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc"]);
    for f in Format::ALL {
        assert_eq!(Format::parse(f.as_str()), Some(f));
    }
    assert_eq!(Format::parse(" GLB "), Some(Glb));
    assert_eq!(Format::parse("3MF"), Some(ThreeMf));
    assert_eq!(Format::parse("USDC"), Some(Usdc));
    // A format name means both directions; `.usd` only reads (as usda, or as
    // usdc when its bytes are a crate).
    assert_eq!(Format::parse("usd"), None);
    assert_eq!(Format::parse(""), None);
}

#[test]
fn formats_lists_all_in_order() {
    let listed: Vec<Format> = formats().iter().map(|i| i.format).collect();
    assert_eq!(listed, Format::ALL.to_vec());
}

#[test]
fn media_types_match_litegens_table() {
    // litegen-core/src/store/mod.rs `model3d_content_type`, with the HTTP
    // converter's application/octet-stream fallback for fbx and ply.
    let want = [
        (Glb, "model/gltf-binary"),
        (Gltf, "model/gltf+json"),
        (Obj, "model/obj"),
        (Stl, "model/stl"),
        (Ply, "application/octet-stream"),
        (ThreeMf, "model/3mf"),
        (Dae, "model/vnd.collada+xml"),
        (Fbx, "application/octet-stream"),
        (Usdz, "model/vnd.usdz+zip"),
        (Usda, "model/vnd.usda"),
        // IANA registers no type for a binary USD crate.
        (Usdc, "application/octet-stream"),
    ];
    for (f, media) in want {
        assert_eq!(f.media_type(), media, "{f:?}");
        assert_eq!(f.info().media_type, media, "{f:?}");
    }
}

/// mesh's capability row, mapped by hand.
fn keeps_from(c: litegen_mesh::Capabilities) -> Keeps {
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
    let mut kept = Vec::new();
    for (on, layer) in [
        (clearcoat, MaterialLayer::Clearcoat),
        (transmission, MaterialLayer::Transmission),
        (volume, MaterialLayer::Volume),
        (ior, MaterialLayer::Ior),
        (specular, MaterialLayer::Specular),
        (sheen, MaterialLayer::Sheen),
        (iridescence, MaterialLayer::Iridescence),
        (anisotropy, MaterialLayer::Anisotropy),
        (dispersion, MaterialLayer::Dispersion),
    ] {
        if on {
            kept.push(layer);
        }
    }
    Keeps {
        hierarchy,
        normals,
        uv_sets: uv_sets as u32,
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
        layers: kept,
        lights,
        cameras,
        points,
        lines,
    }
}

fn mesh_format(f: Format) -> MeshFormat {
    MeshFormat::parse(f.as_str()).unwrap()
}

#[test]
fn keeps_mirror_mesh_capabilities() {
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
    for f in Format::ALL {
        let info = f.info();
        assert_eq!(info.keeps, keeps_from(mesh_format(f).capabilities(file)), "{f:?} as one file");
        assert_eq!(info.keeps_in_package, keeps_from(mesh_format(f).capabilities(package)), "{f:?} as a package");
    }
    let stl = Stl.info().keeps;
    assert!(!stl.normals && !stl.textures && stl.materials == MaterialSupport::None && stl.uv_sets == 0);
    assert_eq!(Obj.info().keeps.materials, MaterialSupport::VertexColor);
    assert_eq!(Obj.info().keeps_in_package.materials, MaterialSupport::Phong);
    assert_eq!(Glb.info().keeps.layers.len(), 9);
    assert!(Fbx.info().keeps.skins && Fbx.info().keeps.animation);
}

#[test]
fn stored_frames() {
    for (f, frame) in [
        (Glb, StoredFrame::YUp),
        (Gltf, StoredFrame::YUp),
        (ThreeMf, StoredFrame::ZUp),
        (Dae, StoredFrame::Declared),
        (Fbx, StoredFrame::Declared),
        (Usdz, StoredFrame::Declared),
        (Usda, StoredFrame::Declared),
        (Usdc, StoredFrame::Declared),
        (Stl, StoredFrame::Caller),
        (Obj, StoredFrame::Caller),
        (Ply, StoredFrame::Caller),
    ] {
        assert_eq!(f.info().stored_frame, frame, "{f:?}");
    }
}

#[test]
fn option_lists() {
    use OptionName::*;
    for f in [Stl, Obj, Ply] {
        assert_eq!(f.info().read_options, [FromUnits, FromUp], "{f:?}");
        assert_eq!(f.info().write_options, [Units, Up], "{f:?}");
    }
    assert_eq!(Fbx.info().read_options, [FbxColorSpace, FbxLightUnits]);
    assert_eq!(Fbx.info().write_options, [FbxColorSpace, FbxLightUnits, Fps]);
    for f in [Usdz, Usda, Usdc] {
        assert_eq!(f.info().read_options, [UsdLightUnits], "{f:?}");
    }
    assert_eq!(Usdz.info().write_options, [UsdLightUnits, UsdzRoot, UsdClips, Fps]);
    for f in [Usda, Usdc] {
        assert_eq!(f.info().write_options, [UsdLightUnits, UsdClips, Fps], "{f:?}");
    }
    assert!(Dae.info().read_options.is_empty());
    assert_eq!(Dae.info().write_options, [DaeMorphs, Fps]);
    for f in [Glb, Gltf, ThreeMf] {
        assert!(f.info().read_options.is_empty() && f.info().write_options.is_empty(), "{f:?}");
    }
}

#[test]
fn extensions_and_sidecars() {
    // `.usd` names either encoding, so both list it; the bytes decide.
    assert_eq!(Usda.info().extensions, ["usda", "usd"]);
    assert_eq!(Usdc.info().extensions, ["usdc", "usd"]);
    for f in Format::ALL.into_iter().filter(|f| !matches!(f, Usda | Usdc)) {
        assert_eq!(f.info().extensions, [f.as_str()], "{f:?}");
    }
    let sidecars: Vec<Format> = Format::ALL.into_iter().filter(|f| f.info().writes_sidecars).collect();
    assert_eq!(sidecars, [Gltf, Obj, Dae, Usda, Usdc]);
}

#[test]
fn labels_name_each_container() {
    let labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
    assert_eq!(
        labels,
        ["glTF Binary", "glTF", "Wavefront OBJ", "STL", "PLY", "3MF", "COLLADA", "FBX", "USDZ", "USD (text)", "USD (binary)"]
    );
}

#[test]
fn closed_set_spellings_and_parsing() {
    assert_eq!(Unit::ALL.map(Unit::as_str), ["m", "cm", "mm", "in", "ft"]);
    assert_eq!(UpAxis::ALL.map(UpAxis::as_str), ["y", "z"]);
    assert_eq!(FbxColorSpace::ALL.map(FbxColorSpace::as_str), ["srgb", "linear"]);
    assert_eq!(LightUnits::ALL.map(LightUnits::as_str), ["photometric", "blender"]);
    assert_eq!(DaeMorphs::ALL.map(DaeMorphs::as_str), ["bake", "keep"]);
    assert_eq!(UsdzRoot::ALL.map(UsdzRoot::as_str), ["usda", "usdc"]);
    assert_eq!(UsdClips::ALL.map(UsdClips::as_str), ["all", "first"]);
    assert_eq!(Packaging::ALL.map(Packaging::as_str), ["file", "zip", "files"]);
    assert_eq!(Unit::parse(" MM "), Some(Unit::Mm));
    assert_eq!(Unit::parse("km"), None);
    assert_eq!(UpAxis::parse("Z"), Some(UpAxis::Z));
    assert_eq!(UpAxis::parse("x"), None);
    // As litegen's HTTP API takes them (parse_query): a signed axis too.
    assert_eq!(UpAxis::parse("+z"), Some(UpAxis::Z));
    assert_eq!(UpAxis::parse(" +Y "), Some(UpAxis::Y));
    assert_eq!(UpAxis::parse("-z"), None);
    assert_eq!(FbxColorSpace::parse("Linear"), Some(FbxColorSpace::Linear));
    // Case- and space-insensitive, as litegen's HTTP API takes them.
    assert_eq!(LightUnits::parse(" Blender "), Some(LightUnits::Blender));
    assert_eq!(LightUnits::parse("PHOTOMETRIC"), Some(LightUnits::Photometric));
    assert_eq!(LightUnits::parse("watts"), None);
    assert_eq!(DaeMorphs::parse(" Keep "), Some(DaeMorphs::Keep));
    assert_eq!(DaeMorphs::parse("BAKE"), Some(DaeMorphs::Bake));
    assert_eq!(DaeMorphs::parse("baked"), None);
    assert_eq!(UsdzRoot::parse(" USDC "), Some(UsdzRoot::Usdc));
    assert_eq!(UsdzRoot::parse("Usda"), Some(UsdzRoot::Usda));
    assert_eq!(UsdzRoot::parse("usd"), None);
    assert_eq!(UsdClips::parse(" All "), Some(UsdClips::All));
    assert_eq!(UsdClips::parse("FIRST"), Some(UsdClips::First));
    assert_eq!(UsdClips::parse("none"), None);
    assert_eq!(Packaging::parse("ZIP"), Some(Packaging::Zip));
    assert_eq!(Packaging::parse("tar"), None);
    assert_eq!(
        OptionName::ALL.map(OptionName::as_str),
        [
            "from", "to", "file_name", "package", "units", "up", "from_units", "from_up", "fbx_color_space", "fbx_light_units",
            "usd_light_units", "dae_morphs", "usdz_root", "usd_clips", "fps",
        ]
    );
}

/// `usdz_root` and `usd_clips` are mesh's choices, spelled as mesh spells
/// them. `usdz_root`'s default is mesh's; `usd_clips` has none of its own, so
/// an absent one stays absent and mesh picks the container's.
#[test]
fn usd_choices_match_mesh() {
    for (ours, mesh) in [(UsdzRoot::Usda, litegen_mesh::UsdzRoot::Usda), (UsdzRoot::Usdc, litegen_mesh::UsdzRoot::Usdc)] {
        assert_eq!(litegen_mesh::UsdzRoot::from(ours), mesh, "{ours:?}");
        assert_eq!(ours.as_str(), mesh.as_str(), "{ours:?}");
    }
    assert_eq!(litegen_mesh::UsdzRoot::from(UsdzRoot::Usda), litegen_mesh::UsdzRoot::default());
    for (ours, mesh) in [(UsdClips::All, litegen_mesh::UsdClips::All), (UsdClips::First, litegen_mesh::UsdClips::First)] {
        assert_eq!(litegen_mesh::UsdClips::from(ours), mesh, "{ours:?}");
        assert_eq!(ours.as_str(), mesh.as_str(), "{ours:?}");
    }
}

/// `dae_morphs` is mesh's choice, spelled as mesh spells it, and its default
/// is mesh's.
#[test]
fn dae_morphs_match_mesh() {
    for (ours, mesh) in [(DaeMorphs::Bake, litegen_mesh::DaeMorphs::Bake), (DaeMorphs::Keep, litegen_mesh::DaeMorphs::Keep)] {
        assert_eq!(litegen_mesh::DaeMorphs::from(ours), mesh, "{ours:?}");
        assert_eq!(ours.as_str(), mesh.as_str(), "{ours:?}");
    }
    assert_eq!(litegen_mesh::DaeMorphs::from(DaeMorphs::Bake), litegen_mesh::DaeMorphs::default());
}

#[test]
fn units_match_mesh() {
    for u in Unit::ALL {
        assert_eq!(u.metres(), litegen_mesh::normalize::metres_per_unit(u.as_str()).unwrap(), "{u:?}");
    }
    assert_eq!(Unit::Mm.metres(), 0.001);
    assert_eq!(Unit::In.metres(), 0.0254);
}

#[test]
fn mesh_errors_map_to_api_codes() {
    let cases = [
        (MeshError::NotThisFormat { expected: MeshFormat::Glb, detail: "bad magic".into() }, ErrorCode::FormatMismatch),
        (MeshError::Malformed("short buffer".into()), ErrorCode::MeshMalformed),
        (MeshError::Unsupported("too many keys".into()), ErrorCode::MeshUnsupported),
        (MeshError::Empty, ErrorCode::MeshEmpty),
    ];
    for (mesh_error, code) in cases {
        let text = mesh_error.to_string();
        let e = litegen_3d_converter::Error::from(mesh_error);
        assert_eq!(e.code, code);
        assert_eq!(e.param, None);
        assert_eq!(e.message(), text);
        assert_eq!(e.to_string(), text);
    }
}

#[test]
fn error_codes_spell_like_the_api() {
    use ErrorCode::*;
    // A new code fails to compile here until it is listed below too.
    let _every_code_is_listed = |c: ErrorCode| match c {
        FormatUnrecognized | FormatMismatch | InputAmbiguous | InputTooLarge | MeshMalformed | MeshUnsupported | MeshEmpty | ParamUnsupported
        | ParamInvalid => (),
    };
    assert_eq!(
        ErrorCode::ALL.map(ErrorCode::as_str),
        [
            "format_unrecognized",
            "format_mismatch",
            "input_ambiguous",
            "input_too_large",
            "mesh_malformed",
            "mesh_unsupported",
            "mesh_empty",
            "param_unsupported",
            "param_invalid",
        ]
    );
}

#[test]
fn loss_kinds_spell_like_mesh() {
    use litegen_mesh::LossKind as M;
    // A new mesh kind fails to compile here until it is listed below too.
    let _every_kind_is_listed = |k: M| match k {
        M::Animation
        | M::Skins
        | M::MorphTargets
        | M::Normals
        | M::Uvs
        | M::VertexColors
        | M::Textures
        | M::ImageFormats
        | M::Materials
        | M::Alpha
        | M::Hierarchy
        | M::Meshes
        | M::TextureSampling
        | M::TextureTransforms
        | M::Geometry
        | M::Lights
        | M::Cameras
        | M::Points
        | M::Lines => (),
    };
    let all = [
        M::Animation,
        M::Skins,
        M::MorphTargets,
        M::Normals,
        M::Uvs,
        M::VertexColors,
        M::Textures,
        M::ImageFormats,
        M::Materials,
        M::Alpha,
        M::Hierarchy,
        M::Meshes,
        M::TextureSampling,
        M::TextureTransforms,
        M::Geometry,
        M::Lights,
        M::Cameras,
        M::Points,
        M::Lines,
    ];
    // mesh -> core keeps the kind (its spelling is mesh's own) ...
    for m in all {
        assert_eq!(LossKind::from(m).as_str(), m.as_str(), "{m:?}");
    }
    // ... and core -> mesh -> core is the identity, for every core kind.
    for k in LossKind::ALL {
        assert_eq!(LossKind::from(litegen_mesh::LossKind::from(k)), k, "{k:?}");
    }
    assert_eq!(
        LossKind::ALL.map(LossKind::as_str),
        [
            "animation",
            "skins",
            "morph_targets",
            "normals",
            "uvs",
            "vertex_colors",
            "textures",
            "image_formats",
            "materials",
            "alpha",
            "hierarchy",
            "meshes",
            "texture_sampling",
            "texture_transforms",
            "geometry",
            "lights",
            "cameras",
            "points",
            "lines",
        ]
    );
}

#[test]
fn bind_poses_map_one_to_one() {
    use litegen_mesh::inspect::BindPose as M;
    for (m, want) in [(M::T, BindPose::T), (M::A, BindPose::A), (M::Unknown, BindPose::Unknown)] {
        assert_eq!(BindPose::from(m), want, "{m:?}");
        assert_eq!(BindPose::from(m).as_str(), m.as_str(), "{m:?}");
    }
}

#[test]
fn error_messages_spell_options_per_binding() {
    let e = litegen_3d_converter::Error::new(ErrorCode::ParamUnsupported, Some(OptionName::FromUnits))
        .option(OptionName::FromUnits)
        .text(" applies when ")
        .option(OptionName::From)
        .text(" is stl, obj or ply");
    let ts = |o: OptionName| match o {
        OptionName::FromUnits => "fromUnits",
        OptionName::From => "from",
        _ => "?",
    };
    let py = |o: OptionName| match o {
        OptionName::FromUnits => "from_units",
        OptionName::From => "from_format",
        _ => "?",
    };
    assert_eq!(e.message_with(ts), "`fromUnits` applies when `from` is stl, obj or ply");
    assert_eq!(e.message_with(py), "`from_units` applies when `from_format` is stl, obj or ply");
    assert_eq!(e.to_string(), "`from_units` applies when `from` is stl, obj or ply");
}

/// Every closed set lists each of its values once, in a stable order, spelled
/// as the bindings expose them. The exhaustive `match`es fail to compile when
/// a variant is added, so the list and its spellings are extended with it.
#[test]
fn every_closed_set_lists_its_values() {
    use litegen_3d_converter::{BindPose, ErrorCode, FileRole, LossKind, MaterialLayer, MaterialSupport, StoredFrame};
    let _ = |v: FileRole| match v {
        FileRole::Model | FileRole::MaterialLibrary | FileRole::Buffer | FileRole::Texture | FileRole::Layer | FileRole::Unused => (),
    };
    assert_eq!(FileRole::ALL.map(FileRole::as_str), ["model", "material_library", "buffer", "texture", "layer", "unused"]);
    let _ = |v: StoredFrame| match v {
        StoredFrame::YUp | StoredFrame::ZUp | StoredFrame::Declared | StoredFrame::Caller => (),
    };
    assert_eq!(StoredFrame::ALL.map(StoredFrame::as_str), ["y_up", "z_up", "declared", "caller"]);
    let _ = |v: MaterialSupport| match v {
        MaterialSupport::None | MaterialSupport::VertexColor | MaterialSupport::ColorOnly | MaterialSupport::Phong | MaterialSupport::Pbr => (),
    };
    assert_eq!(MaterialSupport::ALL.map(MaterialSupport::as_str), ["none", "vertex_color", "color_only", "phong", "pbr"]);
    let _ = |v: MaterialLayer| match v {
        MaterialLayer::Clearcoat
        | MaterialLayer::Transmission
        | MaterialLayer::Volume
        | MaterialLayer::Ior
        | MaterialLayer::Specular
        | MaterialLayer::Sheen
        | MaterialLayer::Iridescence
        | MaterialLayer::Anisotropy
        | MaterialLayer::Dispersion => (),
    };
    assert_eq!(
        MaterialLayer::ALL.map(MaterialLayer::as_str),
        ["clearcoat", "transmission", "volume", "ior", "specular", "sheen", "iridescence", "anisotropy", "dispersion"]
    );
    let _ = |v: BindPose| match v {
        BindPose::T | BindPose::A | BindPose::Unknown => (),
    };
    assert_eq!(BindPose::ALL.map(BindPose::as_str), ["t", "a", "unknown"]);
    assert_eq!(ErrorCode::ALL.len(), 9);
    assert_eq!(ErrorCode::ALL[0].as_str(), "format_unrecognized");
    assert_eq!(ErrorCode::ALL[8].as_str(), "param_invalid");
    assert_eq!(LossKind::ALL.len(), 19);
    assert_eq!(LossKind::Lights.as_str(), "lights");
    assert_eq!(LossKind::Cameras.as_str(), "cameras");
    assert_eq!(LossKind::Points.as_str(), "points");
    assert_eq!(LossKind::Lines.as_str(), "lines");
    for (i, k) in LossKind::ALL.iter().enumerate() {
        assert!(!LossKind::ALL[..i].contains(k), "{k:?} listed twice");
    }
    assert_eq!(LossKind::ALL[2].as_str(), "morph_targets");
}
