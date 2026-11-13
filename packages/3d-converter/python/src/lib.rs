//! The PyPI package `litegen-3d-converter` (`import litegen_3d_converter`):
//! litegen's 3D converter as a native extension. A PyO3 layer over
//! `litegen-3d-converter` that parses Python arguments into the core's
//! options, calls it with the GIL released, and wraps its results; every rule
//! lives in the core and `litegen-mesh`.

mod aliases;
mod classes;
mod error;
mod names;

use ::litegen_3d_converter as core;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedBytes;
use pyo3::types::{PyBytes, PyMapping, PyString};

pub use classes::{
    AnimationInfo, Bounds, Conversion, FormatInfo, Humanoid, HumanoidBone, InputFileInfo, Keeps, Loss, MaterialInfo, ModelInfo, OutputFile, SkinInfo,
};
pub use error::ConvertError;
use names::{
    BytesLikeArg, DaeMorphsArg, FbxColorSpaceArg, FormatArg, FpsArg, LightUnitsArg, ModelInputArg, PackagingArg, PyFormat, UnitArg, UpAxisArg,
    UsdClipsArg, UsdzRootArg,
};

#[cfg(feature = "stubgen")]
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pyfunction, gen_stub_pymethods};

/// The module name the stub types are qualified by.
#[cfg(feature = "stubgen")]
pub(crate) const MODULE: &str = "litegen_3d_converter";

#[cfg(feature = "stubgen")]
pyo3_stub_gen::define_stub_info_gatherer!(stub_info);
#[cfg(feature = "stubgen")]
pyo3_stub_gen::module_variable!("litegen_3d_converter", "MAX_INPUT_BYTES", usize);
#[cfg(feature = "stubgen")]
pyo3_stub_gen::module_variable!("litegen_3d_converter", "__version__", String);

/// A buffer argument before any copy: its size is known and its bytes not
/// yet taken, so the core's cap is checked first.
enum Source<'py> {
    /// A `bytes` object, read in place: nothing can change it.
    Bytes(PyBackedBytes),
    /// Any other buffer, as a memoryview, with its size. Python code on
    /// another thread may write a bytearray or a memoryview while the GIL is
    /// released, and Rust must not read memory that is being written, so it
    /// is copied, once, by `tobytes()`. (The buffer protocol is in the
    /// limited API only from Python 3.11, past this abi3-py310 wheel.)
    View(Bound<'py, PyAny>, usize),
}

impl<'py> Source<'py> {
    fn of(obj: &Bound<'py, PyAny>, what: &str) -> PyResult<Source<'py>> {
        let py = obj.py();
        // Only `bytes` itself: PyBackedBytes would copy a bytearray here,
        // before the cap is checked.
        if let Ok(bytes) = obj.cast::<PyBytes>() {
            return Ok(Source::Bytes(PyBackedBytes::from(bytes.clone())));
        }
        let invalid = || error::raise(py, "param_invalid", &format!("`{what}` must be bytes or a bytes-like object"), Some("data"));
        let view = py.import("builtins")?.getattr("memoryview")?.call1((obj,)).map_err(|_| invalid())?;
        let size = view.getattr("nbytes")?.extract::<usize>()?;
        Ok(Source::View(view, size))
    }

    fn len(&self) -> usize {
        match self {
            Source::Bytes(b) => b.len(),
            Source::View(_, size) => *size,
        }
    }

    fn take(self) -> PyResult<PyBackedBytes> {
        match self {
            Source::Bytes(b) => Ok(b),
            Source::View(view, _) => Ok(view.call_method0("tobytes")?.extract::<PyBackedBytes>()?),
        }
    }
}

/// The input as the core takes it: one file, or named files.
enum Data {
    One(PyBackedBytes),
    Many(Vec<(String, PyBackedBytes)>),
}

impl Data {
    /// Run `f` on the input as the core takes it, the files borrowed.
    fn with_core<R>(&self, f: impl FnOnce(core::Input<'_>) -> R) -> R {
        match self {
            Data::One(bytes) => f(core::Input::Bytes(bytes)),
            Data::Many(files) => {
                let files: Vec<core::InputFile<'_>> = files.iter().map(|(name, data)| core::InputFile { name: name.clone(), data: (**data).into() }).collect();
                f(core::Input::Files(&files))
            }
        }
    }
}

/// The core's cap, checked on the sizes before anything is copied.
fn check_size(py: Python<'_>, total: usize) -> PyResult<()> {
    core::check_input_size(total).map_err(|e| error::from_core(py, e))
}

fn data_of(obj: &Bound<'_, PyAny>) -> PyResult<Data> {
    let py = obj.py();
    if !obj.is_instance_of::<PyString>() {
        if let Ok(mapping) = obj.cast::<PyMapping>() {
            let mut sources = Vec::new();
            for item in mapping.items()?.iter() {
                let (name, value): (String, Bound<'_, PyAny>) = item.extract().map_err(|_| {
                    error::raise(py, "param_invalid", "`data`, as a mapping, must map file names (str) to bytes", Some("data"))
                })?;
                let source = Source::of(&value, &format!("data[{name:?}]"))?;
                sources.push((name, source));
            }
            check_size(py, sources.iter().map(|(_, s)| s.len()).fold(0, usize::saturating_add))?;
            let files = sources.into_iter().map(|(name, source)| Ok((name, source.take()?))).collect::<PyResult<_>>()?;
            return Ok(Data::Many(files));
        }
        if let Ok(source) = Source::of(obj, "data") {
            check_size(py, source.len())?;
            return Ok(Data::One(source.take()?));
        }
    }
    Err(error::raise(py, "param_invalid", "`data` must be bytes, a bytes-like object, or a mapping of file names to bytes", Some("data")))
}

/// A closed-set argument, parsed by the core's own parser.
fn parse<T>(py: Python<'_>, value: &str, param: &str, parse: fn(&str) -> Option<T>, takes: &[&str]) -> PyResult<T> {
    parse(value).ok_or_else(|| error::raise(py, "param_invalid", &format!("`{param}` is {value:?}; it takes {}", takes.join(", ")), Some(param)))
}

fn optional<A: AsRef<str>, T>(py: Python<'_>, value: Option<&A>, param: &str, p: fn(&str) -> Option<T>, takes: &[&str]) -> PyResult<Option<T>> {
    value.map(|v| parse(py, v.as_ref(), param, p, takes)).transpose()
}

const FORMATS: [&str; 11] = ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc"];

/// A light-units option, `fbx_light_units` or `usd_light_units`.
fn light_units(py: Python<'_>, value: Option<&LightUnitsArg>, param: &str) -> PyResult<Option<core::LightUnits>> {
    optional(py, value, param, core::LightUnits::parse, &core::LightUnits::ALL.map(core::LightUnits::as_str))
}

#[allow(clippy::too_many_arguments)]
fn read_options(
    py: Python<'_>,
    from_format: Option<&FormatArg>,
    file_name: Option<String>,
    from_units: Option<&UnitArg>,
    from_up: Option<&UpAxisArg>,
    fbx_color_space: Option<&FbxColorSpaceArg>,
    fbx_light_units: Option<&LightUnitsArg>,
    usd_light_units: Option<&LightUnitsArg>,
) -> PyResult<core::ReadOptions> {
    Ok(core::ReadOptions {
        from: optional(py, from_format, "from_format", core::Format::parse, &FORMATS)?,
        file_name,
        from_units: optional(py, from_units, "from_units", core::Unit::parse, &core::Unit::ALL.map(core::Unit::as_str))?,
        from_up: optional(py, from_up, "from_up", core::UpAxis::parse, &core::UpAxis::ALL.map(core::UpAxis::as_str))?,
        fbx_color_space: optional(py, fbx_color_space, "fbx_color_space", core::FbxColorSpace::parse, &core::FbxColorSpace::ALL.map(core::FbxColorSpace::as_str))?,
        fbx_light_units: light_units(py, fbx_light_units, "fbx_light_units")?,
        usd_light_units: light_units(py, usd_light_units, "usd_light_units")?,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_options(
    py: Python<'_>,
    to_format: &FormatArg,
    package: Option<&PackagingArg>,
    units: Option<&UnitArg>,
    up: Option<&UpAxisArg>,
    fbx_color_space: Option<&FbxColorSpaceArg>,
    fbx_light_units: Option<&LightUnitsArg>,
    usd_light_units: Option<&LightUnitsArg>,
    dae_morphs: Option<&DaeMorphsArg>,
    usdz_root: Option<&UsdzRootArg>,
    usd_clips: Option<&UsdClipsArg>,
    fps: Option<FpsArg>,
) -> PyResult<core::WriteOptions> {
    let fps = fps
        .map(|FpsArg(f)| {
            u32::try_from(f).map_err(|_| {
                error::raise(py, "param_invalid", &format!("`fps` is {f}; it takes a whole number of frames per second, 1 or more"), Some("fps"))
            })
        })
        .transpose()?;
    Ok(core::WriteOptions {
        to: parse(py, &to_format.0, "to_format", core::Format::parse, &FORMATS)?,
        package: optional(py, package, "package", core::Packaging::parse, &core::Packaging::ALL.map(core::Packaging::as_str))?.unwrap_or(core::Packaging::File),
        units: optional(py, units, "units", core::Unit::parse, &core::Unit::ALL.map(core::Unit::as_str))?,
        up: optional(py, up, "up", core::UpAxis::parse, &core::UpAxis::ALL.map(core::UpAxis::as_str))?,
        fbx_color_space: optional(py, fbx_color_space, "fbx_color_space", core::FbxColorSpace::parse, &core::FbxColorSpace::ALL.map(core::FbxColorSpace::as_str))?,
        fbx_light_units: light_units(py, fbx_light_units, "fbx_light_units")?,
        usd_light_units: light_units(py, usd_light_units, "usd_light_units")?,
        dae_morphs: optional(py, dae_morphs, "dae_morphs", core::DaeMorphs::parse, &core::DaeMorphs::ALL.map(core::DaeMorphs::as_str))?,
        usdz_root: optional(py, usdz_root, "usdz_root", core::UsdzRoot::parse, &core::UsdzRoot::ALL.map(core::UsdzRoot::as_str))?,
        usd_clips: optional(py, usd_clips, "usd_clips", core::UsdClips::parse, &core::UsdClips::ALL.map(core::UsdClips::as_str))?,
        fps,
    })
}

/// Convert a model to another format, in one call.
///
/// `data` is one file's bytes, or a mapping of file names to bytes (the model
/// and the files it references, such as an OBJ's `.mtl` and textures). A zip
/// of a package is unpacked. `package` is `"file"` (the default), `"zip"` or
/// `"files"`. The GIL is released while the converter works.
///
/// Raises `ConvertError` with the API's error `code`.
#[cfg_attr(feature = "stubgen", gen_stub_pyfunction)]
#[pyfunction]
#[pyo3(signature = (data, *, to_format, from_format = None, file_name = None, package = None, units = None, up = None, from_units = None, from_up = None, fbx_color_space = None, fbx_light_units = None, usd_light_units = None, dae_morphs = None, usdz_root = None, usd_clips = None, fps = None))]
#[allow(clippy::too_many_arguments)]
pub fn convert(
    py: Python<'_>,
    data: ModelInputArg<'_>,
    to_format: FormatArg,
    from_format: Option<FormatArg>,
    file_name: Option<String>,
    package: Option<PackagingArg>,
    units: Option<UnitArg>,
    up: Option<UpAxisArg>,
    from_units: Option<UnitArg>,
    from_up: Option<UpAxisArg>,
    fbx_color_space: Option<FbxColorSpaceArg>,
    fbx_light_units: Option<LightUnitsArg>,
    usd_light_units: Option<LightUnitsArg>,
    dae_morphs: Option<DaeMorphsArg>,
    usdz_root: Option<UsdzRootArg>,
    usd_clips: Option<UsdClipsArg>,
    fps: Option<FpsArg>,
) -> PyResult<Conversion> {
    let (fbx_units, usd_units) = (fbx_light_units.as_ref(), usd_light_units.as_ref());
    let read = read_options(py, from_format.as_ref(), file_name, from_units.as_ref(), from_up.as_ref(), fbx_color_space.as_ref(), fbx_units, usd_units)?;
    let write = write_options(
        py,
        &to_format,
        package.as_ref(),
        units.as_ref(),
        up.as_ref(),
        fbx_color_space.as_ref(),
        fbx_units,
        usd_units,
        dae_morphs.as_ref(),
        usdz_root.as_ref(),
        usd_clips.as_ref(),
        fps,
    )?;
    let data = data_of(&data.0)?;
    let converted = py.detach(|| data.with_core(|input| core::convert(input, &read, &write)));
    Conversion::new(py, converted.map_err(|e| error::from_core(py, e))?)
}

/// Read a model once, to inspect it and to write it to as many formats as
/// needed.
///
/// `data` is one file's bytes, or a mapping of file names to bytes. The GIL is
/// released while the converter works.
///
/// Raises `ConvertError` with the API's error `code`.
#[cfg_attr(feature = "stubgen", gen_stub_pyfunction)]
#[pyfunction]
#[pyo3(signature = (data, *, from_format = None, file_name = None, from_units = None, from_up = None, fbx_color_space = None, fbx_light_units = None, usd_light_units = None))]
#[allow(clippy::too_many_arguments)]
pub fn read_model(
    py: Python<'_>,
    data: ModelInputArg<'_>,
    from_format: Option<FormatArg>,
    file_name: Option<String>,
    from_units: Option<UnitArg>,
    from_up: Option<UpAxisArg>,
    fbx_color_space: Option<FbxColorSpaceArg>,
    fbx_light_units: Option<LightUnitsArg>,
    usd_light_units: Option<LightUnitsArg>,
) -> PyResult<Model> {
    let options = read_options(
        py,
        from_format.as_ref(),
        file_name,
        from_units.as_ref(),
        from_up.as_ref(),
        fbx_color_space.as_ref(),
        fbx_light_units.as_ref(),
        usd_light_units.as_ref(),
    )?;
    let data = data_of(&data.0)?;
    let inner = py.detach(|| data.with_core(|input| core::Model::read(input, &options))).map_err(|e| error::from_core(py, e))?;
    Ok(Model { inner })
}

/// The format of a file, from its bytes and, when they cannot tell, its name.
/// `None` when neither says. Raises ConvertError (`input_too_large`) past
/// MAX_INPUT_BYTES, before a buffer is copied.
#[cfg_attr(feature = "stubgen", gen_stub_pyfunction)]
#[pyfunction]
#[pyo3(signature = (data, file_name = None))]
pub fn detect_format(data: BytesLikeArg<'_>, file_name: Option<&str>) -> PyResult<Option<PyFormat>> {
    let source = Source::of(&data.0, "data")?;
    check_size(data.0.py(), source.len())?;
    let bytes = source.take()?;
    Ok(core::detect_format(&bytes, file_name).map(PyFormat))
}

/// Every format, what it keeps and which options it takes.
#[cfg_attr(feature = "stubgen", gen_stub_pyfunction)]
#[pyfunction]
pub fn formats() -> Vec<FormatInfo> {
    core::formats().into_iter().map(FormatInfo::from).collect()
}

/// This package's version.
#[cfg_attr(feature = "stubgen", gen_stub_pyfunction)]
#[pyfunction]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A model read once, ready to write to any number of formats. Immutable, so
/// one model can be written from many threads at once.
#[cfg_attr(feature = "stubgen", gen_stub_pyclass)]
#[pyclass(frozen, module = "litegen_3d_converter")]
pub struct Model {
    inner: core::Model,
}

#[cfg_attr(feature = "stubgen", gen_stub_pymethods)]
#[pymethods]
impl Model {
    /// The format it was read as.
    #[getter]
    fn format(&self) -> PyFormat {
        PyFormat(self.inner.format())
    }

    /// What the reader measures in it: counts, bounds, materials, skin and
    /// clips. Measured on first use (reading and writing never measure), by
    /// placing every instance of every mesh, so it raises ConvertError
    /// (`mesh_unsupported`) for a model past 4,000,000 vertices once every
    /// instance is placed, which still converts.
    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<ModelInfo> {
        let info = py.detach(|| self.inner.info().map(ModelInfo::from));
        info.map_err(|e| error::from_core(py, e))
    }

    /// What reading it dropped or approximated, in prose.
    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.inner.warnings().to_vec()
    }

    /// Every input file and the role it played.
    #[getter]
    fn files(&self) -> Vec<InputFileInfo> {
        self.inner.files().iter().map(InputFileInfo::from).collect()
    }

    /// Write the model. `package` is `"file"` (the default), `"zip"` or
    /// `"files"`. The GIL is released while the converter works.
    ///
    /// Raises `ConvertError` with the API's error `code`.
    #[pyo3(signature = (*, to_format, package = None, units = None, up = None, fbx_color_space = None, fbx_light_units = None, usd_light_units = None, dae_morphs = None, usdz_root = None, usd_clips = None, fps = None))]
    #[allow(clippy::too_many_arguments)] // the keyword arguments are the API
    fn write(
        &self,
        py: Python<'_>,
        to_format: FormatArg,
        package: Option<PackagingArg>,
        units: Option<UnitArg>,
        up: Option<UpAxisArg>,
        fbx_color_space: Option<FbxColorSpaceArg>,
        fbx_light_units: Option<LightUnitsArg>,
        usd_light_units: Option<LightUnitsArg>,
        dae_morphs: Option<DaeMorphsArg>,
        usdz_root: Option<UsdzRootArg>,
        usd_clips: Option<UsdClipsArg>,
        fps: Option<FpsArg>,
    ) -> PyResult<Conversion> {
        let options = write_options(
            py,
            &to_format,
            package.as_ref(),
            units.as_ref(),
            up.as_ref(),
            fbx_color_space.as_ref(),
            fbx_light_units.as_ref(),
            usd_light_units.as_ref(),
            dae_morphs.as_ref(),
            usdz_root.as_ref(),
            usd_clips.as_ref(),
            fps,
        )?;
        let written = py.detach(|| self.inner.write(&options));
        Conversion::new(py, written.map_err(|e| error::from_core(py, e))?)
    }

    /// Its format and input files; it measures nothing.
    fn __repr__(&self) -> String {
        format!("Model(format={:?}, files={})", self.inner.format().as_str(), self.inner.files().len())
    }
}

/// litegen's 3D converter: glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda
/// and usdc, every pair, both ways, with what each target cannot hold
/// reported.
#[pymodule(gil_used = false)]
fn litegen_3d_converter(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(convert, m)?)?;
    m.add_function(wrap_pyfunction!(read_model, m)?)?;
    m.add_function(wrap_pyfunction!(detect_format, m)?)?;
    m.add_function(wrap_pyfunction!(formats, m)?)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_class::<Model>()?;
    m.add_class::<Conversion>()?;
    m.add_class::<OutputFile>()?;
    m.add_class::<Loss>()?;
    m.add_class::<ModelInfo>()?;
    m.add_class::<Bounds>()?;
    m.add_class::<MaterialInfo>()?;
    m.add_class::<SkinInfo>()?;
    m.add_class::<Humanoid>()?;
    m.add_class::<HumanoidBone>()?;
    m.add_class::<AnimationInfo>()?;
    m.add_class::<InputFileInfo>()?;
    m.add_class::<FormatInfo>()?;
    m.add_class::<Keeps>()?;
    error::add(m)?;
    aliases::add(m)?;
    m.add("MAX_INPUT_BYTES", core::MAX_INPUT_BYTES)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // Every public name, as the stub's __all__ lists them.
    let mut public: Vec<String> = m.dict().keys().iter().filter_map(|k| k.extract::<String>().ok()).filter(|k| !k.starts_with('_') || k == "__version__").collect();
    public.sort();
    m.add("__all__", public)?;
    Ok(())
}
