//! The `Literal` aliases (`Format`, `Unit`, …): built from the core's own
//! value lists, as real `typing.Literal` objects at runtime (so `from
//! litegen_3d_converter import Format` works) and, with the `stubgen`
//! feature, as `TypeAlias` declarations in the stub.

use ::litegen_3d_converter as core;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

use crate::names::py_name;

/// Each alias: its name, its docstring, and its values in order.
pub(crate) fn all() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        ("Format", "A 3D container the converter reads and writes.", core::Format::ALL.map(core::Format::as_str).to_vec()),
        ("Unit", "A length unit, for the containers with none of their own (STL, OBJ, PLY).", core::Unit::ALL.map(core::Unit::as_str).to_vec()),
        ("UpAxis", "A unitless file's up axis.", core::UpAxis::ALL.map(core::UpAxis::as_str).to_vec()),
        ("FbxColorSpace", "How FBX material colours are stored.", core::FbxColorSpace::ALL.map(core::FbxColorSpace::as_str).to_vec()),
        ("LightUnits", "How FBX and USD light intensities are stored.", core::LightUnits::ALL.map(core::LightUnits::as_str).to_vec()),
        ("DaeMorphs", "How a Collada file holds morph targets.", core::DaeMorphs::ALL.map(core::DaeMorphs::as_str).to_vec()),
        ("UsdzRoot", "What a USDZ package's root layer is written as.", core::UsdzRoot::ALL.map(core::UsdzRoot::as_str).to_vec()),
        ("UsdClips", "Which animation clips a USD file holds.", core::UsdClips::ALL.map(core::UsdClips::as_str).to_vec()),
        ("Packaging", "What a conversion answers with: one file, a zip, or separate files.", core::Packaging::ALL.map(core::Packaging::as_str).to_vec()),
        ("FileRole", "What a file is to its model.", core::FileRole::ALL.map(core::FileRole::as_str).to_vec()),
        ("LossKind", "The kind of content a loss is about.", core::LossKind::ALL.map(core::LossKind::as_str).to_vec()),
        ("ErrorCode", "Why a call failed, as litegen's HTTP API spells it.", core::ErrorCode::ALL.map(core::ErrorCode::as_str).to_vec()),
        ("StoredFrame", "The frame a file's numbers are in.", core::StoredFrame::ALL.map(core::StoredFrame::as_str).to_vec()),
        ("MaterialSupport", "How much of a material model a format keeps.", core::MaterialSupport::ALL.map(core::MaterialSupport::as_str).to_vec()),
        ("MaterialLayer", "A material layer (glTF's KHR_materials_* extensions).", core::MaterialLayer::ALL.map(core::MaterialLayer::as_str).to_vec()),
        ("BindPose", "A skinned humanoid's rest pose.", core::BindPose::ALL.map(core::BindPose::as_str).to_vec()),
        ("TextureMap", "A material texture map.", core::TextureMap::ALL.map(core::TextureMap::as_str).to_vec()),
        ("HumanoidNaming", "A humanoid joint naming scheme litegen knows.", core::HumanoidNaming::ALL.map(core::HumanoidNaming::as_str).to_vec()),
        ("VrmBone", "A VRM 1.0 humanoid bone.", core::VrmBone::ALL.map(core::VrmBone::as_str).to_vec()),
        ("OptionName", "An option's name, as this package spells it.", core::OptionName::ALL.map(py_name).to_vec()),
    ]
}

/// `typing.Literal[values...]`.
pub(crate) fn literal<'py>(py: Python<'py>, values: &[&str]) -> PyResult<Bound<'py, PyAny>> {
    let literal = py.import("typing")?.getattr("Literal")?;
    literal.get_item(PyTuple::new(py, values)?)
}

/// Adds every alias, and `BytesLike` and `ModelInput`, to the module.
pub(crate) fn add(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    for (name, _, values) in all() {
        m.add(name, literal(py, &values)?)?;
    }
    let bytes_like = py.eval(c"bytes | bytearray | memoryview", None, None)?;
    m.add("BytesLike", &bytes_like)?;
    let mapping = py.import("collections.abc")?.getattr("Mapping")?;
    let files = mapping.get_item(PyTuple::new(py, [py.get_type::<pyo3::types::PyString>().into_any(), bytes_like.clone()])?)?;
    m.add("ModelInput", bytes_like.call_method1("__or__", (files,))?)?;
    Ok(())
}

#[cfg(feature = "stubgen")]
pub(crate) mod stub {
    //! The stub's declarations of the aliases: marker types whose stub type
    //! is `typing.Literal[...]` of the core's values.

    use pyo3::prelude::*;
    use pyo3_stub_gen::{PyStubType, TypeInfo};

    fn literal_type(values: &[&str]) -> TypeInfo {
        let quoted: Vec<String> = values.iter().map(|v| format!("{v:?}")).collect();
        TypeInfo {
            name: format!("typing.Literal[{}]", quoted.join(", ")),
            source_module: None,
            import: ["typing".into()].into_iter().collect(),
            type_refs: Default::default(),
        }
    }

    fn values_of(name: &str) -> Vec<&'static str> {
        super::all().into_iter().find(|(n, _, _)| *n == name).map(|(_, _, v)| v).unwrap_or_default()
    }

    macro_rules! literal_alias {
        ($marker:ident, $name:ident, $doc:literal) => {
            pub struct $marker;
            impl PyStubType for $marker {
                fn type_output() -> TypeInfo {
                    literal_type(&values_of(stringify!($name)))
                }
            }
            impl pyo3_stub_gen::runtime::PyRuntimeType for $marker {
                fn runtime_type_object(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
                    super::literal(py, &values_of(stringify!($name)))
                }
            }
            pyo3_stub_gen::type_alias!("litegen_3d_converter", $name = $marker, $doc);
        };
    }

    literal_alias!(FormatValues, Format, "A 3D container the converter reads and writes. Every pair converts, both ways. `usda` writes USDA text and `usdc` a binary crate; each reads either encoding (`.usda`, `.usdc`, or a `.usd` holding either).");
    literal_alias!(UnitValues, Unit, "A length unit, for the containers with none of their own (STL, OBJ, PLY). Slicers read STL as millimetres.");
    literal_alias!(UpAxisValues, UpAxis, "A unitless file's up axis: `y` (glTF's) or `z` (slicers, CAD, Blender), where +Y up becomes +Z up and the front (+Z) becomes -Y.");
    literal_alias!(FbxColorSpaceValues, FbxColorSpace, "How FBX material colours are stored: `srgb` (the default; three.js reads them so) or `linear` (Blender, Unity, Unreal, ufbx and assimp do).");
    literal_alias!(LightUnitsValues, LightUnits, "How FBX and USD light intensities are stored: `photometric` (the default; glTF's candela and lux, as ufbx and three.js read FBX) or `blender` (Blender's watts, so the file opens in Blender as bright as the scene's glTF does).");
    literal_alias!(DaeMorphsValues, DaeMorphs, "How a Collada file holds morph targets: `bake` (the default; each node's mesh at that node's weights, which every Collada reader opens, with the targets and their weight animation reported as losses) or `keep` (`<morph>` controllers with their weight animation, which SceneKit (and so Xcode) and Godot read, while three.js's ColladaLoader and assimp refuse the whole file).");
    literal_alias!(UsdzRootValues, UsdzRoot, "What a USDZ package's root layer, its first entry, is: `usda` (the default; USDA text, which `usdchecker --arkit` accepts and `<model-viewer>` hands to AR Quick Look as it is) or `usdc` (the binary crate, as Apple's own packagers write it). The two read back as one scene.");
    literal_alias!(UsdClipsValues, UsdClips, "Which animation clips a USD file holds: `all` (every clip as an alternative, a variant set `Animation` on the default prim with one variant per clip, the first selected, so a reader that composes variants can switch between them and one that does not plays the first; rigid (node) motion is the first clip's only, the others' is named in the losses, and a scene whose clips are all rigid writes as `first`) or `first` (the first clip alone). When absent, `all` for `usda` and `usdc`, and `first` for `usdz` until AR Quick Look's behaviour with variants is checked on a device.");
    literal_alias!(PackagingValues, Packaging, "What a conversion answers with: `file` (one self-contained file, the default), `zip` (one zip of the file and its sidecars) or `files` (those files separately, the model first).");
    literal_alias!(FileRoleValues, FileRole, "What a file is to its model.");
    literal_alias!(LossKindValues, LossKind, "The kind of content a loss is about, spelled as litegen's HTTP API spells it.");
    literal_alias!(ErrorCodeValues, ErrorCode, "Why a call failed, as litegen's HTTP API spells it.");
    literal_alias!(StoredFrameValues, StoredFrame, "The frame a file's numbers are in: `y_up` (glTF), `z_up` (3MF), `declared` (the file says: Collada, FBX, USD) or `caller` (STL, OBJ, PLY: your `up` option says).");
    literal_alias!(MaterialSupportValues, MaterialSupport, "How much of a material model a format keeps.");
    literal_alias!(MaterialLayerValues, MaterialLayer, "A material layer (glTF's KHR_materials_* extensions).");
    literal_alias!(BindPoseValues, BindPose, "A skinned humanoid's rest pose: `t`, `a` or `unknown`.");
    literal_alias!(TextureMapValues, TextureMap, "A material texture map: glTF's five core maps, and the layer maps of its PBR extensions.");
    literal_alias!(HumanoidNamingValues, HumanoidNaming, "A humanoid joint naming scheme litegen knows: VRM 1.0's own names, Unity's HumanBodyBones, Mixamo's, Meshy's rigging output, or litegen's mock provider's.");
    literal_alias!(VrmBoneValues, VrmBone, "A VRM 1.0 humanoid bone (VRMC_vrm-1.0/humanoid.md), optional bones included.");
    literal_alias!(OptionNameValues, OptionName, "An option's name, as this package spells it.");

    pyo3_stub_gen::derive::gen_type_alias_from_python!(
        "litegen_3d_converter",
        r#"
        import typing_extensions
        from typing import TypeAlias
        BytesLike: TypeAlias = typing_extensions.Buffer
        """One file's bytes: any buffer (bytes, bytearray, memoryview, array.array, mmap, a NumPy array). `bytes` is read in place; any other buffer is copied once."""
        "#
    );
    pyo3_stub_gen::derive::gen_type_alias_from_python!(
        "litegen_3d_converter",
        r#"
        import collections.abc
        from typing import TypeAlias
        ModelInput: TypeAlias = BytesLike | collections.abc.Mapping[str, BytesLike]
        """One model file's bytes, or several files by name: the model and the files it references, such as an OBJ's `.mtl` and textures. A zip of a package is unpacked."""
        "#
    );
}
