//! `ConvertError`: a `ValueError` carrying the API's error `code` and the
//! `param` it is about. A plain exception class (`create_exception!`): under
//! abi3, PyO3 can subclass exceptions as pyclasses only from Python 3.12.

use ::litegen_3d_converter as core;
use pyo3::prelude::*;

use crate::names::py_name;

#[cfg(feature = "stubgen")]
pyo3_stub_gen::create_exception!(
    litegen_3d_converter,
    ConvertError,
    pyo3::exceptions::PyValueError,
    "Raised by every function of this package. `code` says why, as litegen's HTTP API spells it; `param` names the option it is about, when there is one."
);
#[cfg(not(feature = "stubgen"))]
pyo3::create_exception!(
    litegen_3d_converter,
    ConvertError,
    pyo3::exceptions::PyValueError,
    "Raised by every function of this package. `code` says why, as litegen's HTTP API spells it; `param` names the option it is about, when there is one."
);

/// A `ConvertError` with `code` and `param` set.
pub(crate) fn raise(py: Python<'_>, code: &str, message: &str, param: Option<&str>) -> PyErr {
    let err = ConvertError::new_err(message.to_string());
    let value = err.value(py);
    // Setting an attribute on a fresh exception instance cannot fail.
    let _ = value.setattr("code", code);
    let _ = value.setattr("param", param);
    err
}

pub(crate) fn from_core(py: Python<'_>, e: core::Error) -> PyErr {
    raise(py, e.code.as_str(), &e.message_with(py_name), e.param.map(py_name))
}

/// Adds the class. `param` defaults to `None` on the class; `code` has no
/// honest default, so it exists on the instances this module raises only.
pub(crate) fn add(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    let class = py.get_type::<ConvertError>();
    class.setattr("param", py.None())?;
    m.add("ConvertError", class)
}
