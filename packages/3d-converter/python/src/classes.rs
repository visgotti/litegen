//! The result classes: frozen, read-only views of the core's results. Each
//! is built from a core value by an exhaustive destructuring, so a new core
//! field fails this crate's build at the line to extend.

use ::litegen_3d_converter as core;
use pyo3::prelude::*;
use pyo3::types::PyBytes;

use crate::names::{
    PyBindPose, PyFileRole, PyFormat, PyHumanoidNaming, PyLossKind, PyMaterialLayer, PyMaterialSupport, PyOptionName, PyStoredFrame, PyTextureMap,
    PyVrmBone,
};

#[cfg(feature = "stubgen")]
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};

/// One written file.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
pub struct OutputFile {
    /// Its name: `model.<ext>`, `model.<ext>.zip`, or a sidecar's path
    /// (`model.mtl`, `textures/…`), as the model's references name it.
    #[pyo3(get)]
    pub name: String,
    /// Its media type: the target's for the model, the file's own for a
    /// sidecar.
    #[pyo3(get)]
    pub media_type: String,
    /// What it is to the model: the model itself, a material library, a
    /// buffer or a texture.
    #[pyo3(get)]
    pub role: PyFileRole,
    data: Py<PyBytes>,
}

#[cfg_attr(feature = "stubgen", gen_stub_pymethods)]
#[pymethods]
impl OutputFile {
    /// The file's bytes.
    #[getter]
    fn data(&self, py: Python<'_>) -> Py<PyBytes> {
        self.data.clone_ref(py)
    }

    /// Its name, media type, role and size.
    fn __repr__(&self, py: Python<'_>) -> String {
        format!("OutputFile(name={:?}, media_type={:?}, role={:?}, size={})", self.name, self.media_type, self.role.0.as_str(), self.data.bind(py).as_bytes().len())
    }
}

/// One thing the target could not hold.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct Loss {
    /// What kind of thing it was.
    #[pyo3(get)]
    pub kind: PyLossKind,
    /// What was dropped or approximated, e.g. `animation "walk"`.
    #[pyo3(get)]
    pub what: String,
    /// Why the target could not hold it.
    #[pyo3(get)]
    pub why: String,
}

#[cfg_attr(feature = "stubgen", gen_stub_pymethods)]
#[pymethods]
impl Loss {
    /// Its kind, what and why.
    fn __repr__(&self) -> String {
        format!("Loss(kind={:?}, what={:?}, why={:?})", self.kind.0.as_str(), self.what, self.why)
    }
}

/// A written model: its files (the model first), and everything the target
/// could not hold.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
pub struct Conversion {
    /// The written format.
    #[pyo3(get)]
    pub format: PyFormat,
    files: Vec<Py<OutputFile>>,
    /// What the target could not hold, judged from what the input contains.
    #[pyo3(get)]
    pub losses: Vec<Loss>,
    /// What reading the input dropped or approximated, in prose.
    #[pyo3(get)]
    pub warnings: Vec<String>,
}

#[cfg_attr(feature = "stubgen", gen_stub_pymethods)]
#[pymethods]
impl Conversion {
    /// The written files, the model first: one with `package="file"` or
    /// `"zip"`, the model and its sidecars with `"files"`.
    #[getter]
    fn files(&self, py: Python<'_>) -> Vec<Py<OutputFile>> {
        self.files.iter().map(|f| f.clone_ref(py)).collect()
    }

    /// Its format, and how many files and losses it has.
    fn __repr__(&self) -> String {
        format!("Conversion(format={:?}, files={}, losses={})", self.format.0.as_str(), self.files.len(), self.losses.len())
    }
}

impl Conversion {
    /// The core's result, its bytes copied once into Python `bytes`.
    pub(crate) fn new(py: Python<'_>, c: core::Conversion) -> PyResult<Conversion> {
        let core::Conversion { format, files, losses, warnings } = c;
        let files = files
            .into_iter()
            .map(|core::OutputFile { name, media_type, role, data }| {
                Py::new(py, OutputFile { name, media_type: media_type.to_string(), role: PyFileRole(role), data: PyBytes::new(py, &data).unbind() })
            })
            .collect::<PyResult<Vec<_>>>()?;
        let losses = losses.into_iter().map(|core::Loss { kind, what, why }| Loss { kind: PyLossKind(kind), what, why }).collect();
        Ok(Conversion { format: PyFormat(format), files, losses, warnings })
    }
}

/// One input file as the read used it.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct InputFileInfo {
    /// Its name, as given (unnamed bytes are `model.<ext>`).
    #[pyo3(get)]
    pub name: String,
    /// What it was to the model: the model, a file the model referenced, or
    /// `unused`.
    #[pyo3(get)]
    pub role: PyFileRole,
    /// Bytes.
    #[pyo3(get)]
    pub size: u64,
}

impl From<&core::InputFileInfo> for InputFileInfo {
    fn from(f: &core::InputFileInfo) -> InputFileInfo {
        let core::InputFileInfo { name, role, size } = f;
        InputFileInfo { name: name.clone(), role: PyFileRole(*role), size: *size }
    }
}

/// An axis-aligned box, in metres.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct Bounds {
    /// The smallest x, y and z.
    #[pyo3(get)]
    pub min: (f64, f64, f64),
    /// The largest x, y and z.
    #[pyo3(get)]
    pub max: (f64, f64, f64),
}

/// One material.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct MaterialInfo {
    /// Its name, when the file gives one.
    #[pyo3(get)]
    pub name: Option<String>,
    /// The texture maps it uses, in slot order.
    #[pyo3(get)]
    pub maps: Vec<PyTextureMap>,
}

/// One humanoid bone and the joint that is it.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct HumanoidBone {
    /// The bone, as VRM 1.0 names it.
    #[pyo3(get)]
    pub bone: PyVrmBone,
    /// The joint's name in the file.
    #[pyo3(get)]
    pub joint: String,
}

/// A humanoid rig's bones.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct Humanoid {
    /// The naming scheme its joints follow.
    #[pyo3(get)]
    pub naming: PyHumanoidNaming,
    /// Each VRM 1.0 humanoid bone the rig has, in VRM's order, and the joint
    /// that is it.
    #[pyo3(get)]
    pub bones: Vec<HumanoidBone>,
}

/// A model's skin: the joints that deform its mesh.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct SkinInfo {
    /// How many joints it has.
    #[pyo3(get)]
    pub joints: u32,
    /// The joint every other joint descends from.
    #[pyo3(get)]
    pub root: Option<String>,
    /// Its rest pose.
    #[pyo3(get)]
    pub bind_pose: PyBindPose,
    /// Which joint is which humanoid bone, when the joints' names follow a
    /// naming scheme litegen knows.
    #[pyo3(get)]
    pub humanoid: Option<Humanoid>,
}

/// One animation clip.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct AnimationInfo {
    /// Its name, when the file gives one.
    #[pyo3(get)]
    pub name: Option<String>,
    /// How long it lasts, in seconds, from 0 to its last key.
    #[pyo3(get)]
    pub duration_s: f64,
    /// How far the root moves, first key to last, in metres.
    #[pyo3(get)]
    pub root_travel_m: (f64, f64, f64),
    /// Whether the root stays where it starts, horizontally, all clip long.
    #[pyo3(get)]
    pub in_place: bool,
}

/// What a read model holds, measured by the converter's own reader.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct ModelInfo {
    /// Triangles in the model's meshes. A mesh that several nodes place is
    /// counted once.
    #[pyo3(get)]
    pub triangles: u32,
    /// Vertices in the model's meshes, each mesh counted once.
    #[pyo3(get)]
    pub vertices: u32,
    /// Nodes in the scene graph, joints included.
    #[pyo3(get)]
    pub nodes: u32,
    /// Meshes, each counted once however many nodes place it.
    #[pyo3(get)]
    pub meshes: u32,
    /// Images the materials use.
    #[pyo3(get)]
    pub textures: u32,
    /// Most uv sets on any primitive.
    #[pyo3(get)]
    pub uv_sets: u32,
    /// Whether any primitive has vertex colours.
    #[pyo3(get)]
    pub vertex_colors: bool,
    /// World-space bounds at rest, in metres (the bind pose for a skinned
    /// model), every instance placed. None for a model with no positions.
    #[pyo3(get)]
    pub bounds: Option<Bounds>,
    /// Every material, in the file's order.
    #[pyo3(get)]
    pub materials: Vec<MaterialInfo>,
    /// The first skin, when the model has one.
    #[pyo3(get)]
    pub skin: Option<SkinInfo>,
    /// Every animation clip.
    #[pyo3(get)]
    pub animations: Vec<AnimationInfo>,
}

fn tuple3(v: [f64; 3]) -> (f64, f64, f64) {
    (v[0], v[1], v[2])
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
            bounds: bounds.map(|core::Bounds { min, max }| Bounds { min: tuple3(min), max: tuple3(max) }),
            materials: materials
                .iter()
                .map(|core::MaterialInfo { name, maps }| MaterialInfo { name: name.clone(), maps: maps.iter().copied().map(PyTextureMap).collect() })
                .collect(),
            skin: skin.as_ref().map(|core::SkinInfo { joints, root, bind_pose, humanoid }| SkinInfo {
                joints: *joints,
                root: root.clone(),
                bind_pose: PyBindPose(*bind_pose),
                humanoid: humanoid.as_ref().map(|core::Humanoid { naming, bones }| Humanoid {
                    naming: PyHumanoidNaming(*naming),
                    bones: bones.iter().map(|core::HumanoidBone { bone, joint }| HumanoidBone { bone: PyVrmBone(*bone), joint: joint.clone() }).collect(),
                }),
            }),
            animations: animations
                .iter()
                .map(|core::AnimationInfo { name, duration_s, root_travel_m, in_place }| AnimationInfo {
                    name: name.clone(),
                    duration_s: *duration_s,
                    root_travel_m: tuple3(*root_travel_m),
                    in_place: *in_place,
                })
                .collect(),
        }
    }
}

/// What a format keeps: one row of the converter's lossiness table.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct Keeps {
    /// The node hierarchy and its transforms.
    #[pyo3(get)]
    pub hierarchy: bool,
    /// Vertex normals.
    #[pyo3(get)]
    pub normals: bool,
    /// How many uv sets it keeps per mesh.
    #[pyo3(get)]
    pub uv_sets: u32,
    /// Vertex colours.
    #[pyo3(get)]
    pub vertex_colors: bool,
    /// How much of a material it keeps.
    #[pyo3(get)]
    pub materials: PyMaterialSupport,
    /// Texture images.
    #[pyo3(get)]
    pub textures: bool,
    /// Skins: joints and weights.
    #[pyo3(get)]
    pub skins: bool,
    /// Morph targets (blend shapes).
    #[pyo3(get)]
    pub morph_targets: bool,
    /// Animation clips.
    #[pyo3(get)]
    pub animation: bool,
    /// Several meshes, kept apart.
    #[pyo3(get)]
    pub multiple_meshes: bool,
    /// Texture filtering (nearest or linear).
    #[pyo3(get)]
    pub texture_filters: bool,
    /// Texture wrap modes.
    #[pyo3(get)]
    pub texture_wrap: bool,
    /// Texture offset, scale and rotation (KHR_texture_transform).
    #[pyo3(get)]
    pub texture_transforms: bool,
    /// The material layers it keeps.
    #[pyo3(get)]
    pub layers: Vec<PyMaterialLayer>,
    /// Lights (directional, point and spot).
    #[pyo3(get)]
    pub lights: bool,
    /// Cameras (perspective and orthographic).
    #[pyo3(get)]
    pub cameras: bool,
    /// Point primitives (otherwise left out).
    #[pyo3(get)]
    pub points: bool,
    /// Line primitives (otherwise left out).
    #[pyo3(get)]
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
            materials: PyMaterialSupport(materials),
            textures,
            skins,
            morph_targets,
            animation,
            multiple_meshes,
            texture_filters,
            texture_wrap,
            texture_transforms,
            layers: layers.into_iter().map(PyMaterialLayer).collect(),
            lights,
            cameras,
            points,
            lines,
        }
    }
}

/// Everything about a format a caller needs.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
#[derive(Clone)]
pub struct FormatInfo {
    /// The format.
    #[pyo3(get)]
    pub format: PyFormat,
    /// Its human name, e.g. `glTF Binary`.
    #[pyo3(get)]
    pub label: String,
    /// The extensions it reads from, lowercase, no dot.
    #[pyo3(get)]
    pub extensions: Vec<String>,
    /// The media type its model file is sent with.
    #[pyo3(get)]
    pub media_type: String,
    /// Whether, written as a package, it has files beside the model.
    #[pyo3(get)]
    pub writes_sidecars: bool,
    /// The frame its numbers are stored in, for a viewer whose loader does
    /// not convert it.
    #[pyo3(get)]
    pub stored_frame: PyStoredFrame,
    /// What it keeps as one file.
    #[pyo3(get)]
    pub keeps: Keeps,
    /// What it keeps as a package (`zip` or `files`).
    #[pyo3(get)]
    pub keeps_in_package: Keeps,
    /// The read options it takes, beyond `from_format` and `file_name`.
    #[pyo3(get)]
    pub read_options: Vec<PyOptionName>,
    /// The write options it takes, beyond `to_format` and `package`.
    #[pyo3(get)]
    pub write_options: Vec<PyOptionName>,
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
            format: PyFormat(format),
            label: label.to_string(),
            extensions: extensions.into_iter().map(String::from).collect(),
            media_type: media_type.to_string(),
            writes_sidecars,
            stored_frame: PyStoredFrame(stored_frame),
            keeps: keeps.into(),
            keeps_in_package: keeps_in_package.into(),
            read_options: read_options.into_iter().map(PyOptionName).collect(),
            write_options: write_options.into_iter().map(PyOptionName).collect(),
        }
    }
}
