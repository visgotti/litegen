//! Python's spellings: option names (the keyword arguments), and the closed
//! sets as plain `str` values typed by the stub's `Literal` aliases.

use ::litegen_3d_converter as core;
use pyo3::prelude::*;
use pyo3::types::PyString;

/// An option's name as a keyword argument of this package, as litegen's
/// Python SDK spells it (`from_format`, `to_format`).
pub(crate) fn py_name(o: core::OptionName) -> &'static str {
    match o {
        core::OptionName::From => "from_format",
        core::OptionName::To => "to_format",
        core::OptionName::FileName => "file_name",
        core::OptionName::Package => "package",
        core::OptionName::Units => "units",
        core::OptionName::Up => "up",
        core::OptionName::FromUnits => "from_units",
        core::OptionName::FromUp => "from_up",
        core::OptionName::FbxColorSpace => "fbx_color_space",
        core::OptionName::FbxLightUnits => "fbx_light_units",
        core::OptionName::UsdLightUnits => "usd_light_units",
        core::OptionName::DaeMorphs => "dae_morphs",
        core::OptionName::UsdzRoot => "usdz_root",
        core::OptionName::UsdClips => "usd_clips",
        core::OptionName::Fps => "fps",
    }
}

/// A closed-set value: a `str` at runtime, its `Literal` alias in the stub.
macro_rules! literal {
    ($(#[$doc:meta])* $name:ident($core:ty) as $alias:literal, $spell:expr) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $name(pub $core);

        impl<'py> IntoPyObject<'py> for $name {
            type Target = PyString;
            type Output = Bound<'py, PyString>;
            type Error = std::convert::Infallible;

            fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
                let spell: fn($core) -> &'static str = $spell;
                Ok(PyString::intern(py, spell(self.0)))
            }
        }

        impl<'a, 'py> IntoPyObject<'py> for &'a $name {
            type Target = PyString;
            type Output = Bound<'py, PyString>;
            type Error = std::convert::Infallible;

            fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
                (*self).into_pyobject(py)
            }
        }

        #[cfg(feature = "stubgen")]
        impl pyo3_stub_gen::PyStubType for $name {
            fn type_output() -> pyo3_stub_gen::TypeInfo {
                pyo3_stub_gen::TypeInfo::locally_defined($alias, crate::MODULE.into())
            }
        }
    };
}

literal!(PyFormat(core::Format) as "Format", core::Format::as_str);
literal!(PyFileRole(core::FileRole) as "FileRole", core::FileRole::as_str);
literal!(PyLossKind(core::LossKind) as "LossKind", core::LossKind::as_str);
literal!(PyStoredFrame(core::StoredFrame) as "StoredFrame", core::StoredFrame::as_str);
literal!(PyMaterialSupport(core::MaterialSupport) as "MaterialSupport", core::MaterialSupport::as_str);
literal!(PyMaterialLayer(core::MaterialLayer) as "MaterialLayer", core::MaterialLayer::as_str);
literal!(PyBindPose(core::BindPose) as "BindPose", core::BindPose::as_str);
literal!(PyTextureMap(core::TextureMap) as "TextureMap", core::TextureMap::as_str);
literal!(PyHumanoidNaming(core::HumanoidNaming) as "HumanoidNaming", core::HumanoidNaming::as_str);
literal!(PyVrmBone(core::VrmBone) as "VrmBone", core::VrmBone::as_str);
literal!(PyOptionName(core::OptionName) as "OptionName", py_name);

/// An argument whose value is one of a closed set: extracted as a plain
/// `str` (validated where the argument's name is known, for the error), typed
/// in the stub as its `Literal` alias.
macro_rules! arg {
    ($(#[$doc:meta])* $name:ident as $alias:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone)]
        pub struct $name(pub String);

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl<'a, 'py> FromPyObject<'a, 'py> for $name {
            type Error = PyErr;

            fn extract(obj: pyo3::Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
                Ok($name(obj.extract::<String>()?))
            }
        }

        // A default value is shown in the stub as the str it is.
        impl<'py> IntoPyObject<'py> for $name {
            type Target = PyString;
            type Output = Bound<'py, PyString>;
            type Error = std::convert::Infallible;

            fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
                Ok(PyString::new(py, &self.0))
            }
        }

        #[cfg(feature = "stubgen")]
        impl pyo3_stub_gen::PyStubType for $name {
            fn type_output() -> pyo3_stub_gen::TypeInfo {
                pyo3_stub_gen::TypeInfo::locally_defined($alias, crate::MODULE.into())
            }
        }
    };
}

arg!(FormatArg as "Format");
arg!(UnitArg as "Unit");
arg!(UpAxisArg as "UpAxis");
arg!(FbxColorSpaceArg as "FbxColorSpace");
arg!(LightUnitsArg as "LightUnits");
arg!(DaeMorphsArg as "DaeMorphs");
arg!(UsdzRootArg as "UsdzRoot");
arg!(UsdClipsArg as "UsdClips");
arg!(PackagingArg as "Packaging");

/// `fps`: a whole number, and never a bool, which Python counts as an int
/// (`fps=True` would otherwise read as 1). `int` in the stub.
#[derive(Debug, Clone, Copy)]
pub struct FpsArg(pub i64);

impl<'a, 'py> FromPyObject<'a, 'py> for FpsArg {
    type Error = PyErr;

    fn extract(obj: pyo3::Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
        if obj.is_instance_of::<pyo3::types::PyBool>() {
            return Err(pyo3::exceptions::PyTypeError::new_err("`fps` takes an int, not a bool"));
        }
        Ok(FpsArg(obj.extract::<i64>()?))
    }
}

#[cfg(feature = "stubgen")]
impl pyo3_stub_gen::PyStubType for FpsArg {
    fn type_output() -> pyo3_stub_gen::TypeInfo {
        <i64 as pyo3_stub_gen::PyStubType>::type_output()
    }
}

/// The model argument: any object, checked by `data_of`; `ModelInput` (or
/// `BytesLike`) in the stub.
macro_rules! object_arg {
    ($name:ident as $alias:literal) => {
        pub struct $name<'py>(pub Bound<'py, PyAny>);

        impl<'a, 'py> FromPyObject<'a, 'py> for $name<'py> {
            type Error = PyErr;

            fn extract(obj: pyo3::Borrowed<'a, 'py, PyAny>) -> Result<Self, Self::Error> {
                Ok($name(obj.to_owned()))
            }
        }

        #[cfg(feature = "stubgen")]
        impl pyo3_stub_gen::PyStubType for $name<'_> {
            fn type_output() -> pyo3_stub_gen::TypeInfo {
                pyo3_stub_gen::TypeInfo::locally_defined($alias, crate::MODULE.into())
            }
        }
    };
}

object_arg!(ModelInputArg as "ModelInput");
object_arg!(BytesLikeArg as "BytesLike");
