//! Writes `litegen_3d_converter.pyi` from the Rust source (pyo3-stub-gen):
//! `cargo run -p litegen-3d-converter-py --bin stub_gen --features stubgen
//! [-- <dir>]`, into the project folder, or into `<dir>` when given.

/// `ConvertError` is a plain exception class (`create_exception!`), whose
/// attributes pyo3-stub-gen cannot see; they are declared here, as the module
/// sets them on every raised instance (src/error.rs).
const CONVERT_ERROR_ATTRIBUTES: &str = "    code: ErrorCode\n    r\"\"\"Why the call failed, as litegen's HTTP API spells it.\"\"\"\n    param: builtins.str | None\n    r\"\"\"The keyword argument the error is about, when there is one.\"\"\"\n";

/// pyo3-stub-gen's `module_variable!` takes no docstring; the constant's is
/// written after it, as pyright reads a variable's docstring.
const MAX_INPUT_BYTES_DOC: &str = "r\"\"\"Largest input the converter reads, in bytes (128 MiB): the files' total, and a zip's unpacked size.\"\"\"\n";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut info = litegen_3d_converter_py::stub_info()?;
    if let Some(dir) = std::env::args().nth(1) {
        info.python_root = dir.into();
    }
    info.generate()?;
    let path = info.python_root.join("litegen_3d_converter.pyi");
    let stub = std::fs::read_to_string(&path)?;
    let class = stub.find("class ConvertError(builtins.ValueError):").ok_or("the stub has no ConvertError class")?;
    let body = class + stub[class..].find("\n    ...\n").ok_or("ConvertError's stub has no `...` body")?;
    let patched = format!("{}\n{}{}", &stub[..body], CONVERT_ERROR_ATTRIBUTES, &stub[body + "\n    ...\n".len()..]);
    let constant = "MAX_INPUT_BYTES: builtins.int\n";
    let at = patched.find(constant).ok_or("the stub has no MAX_INPUT_BYTES")? + constant.len();
    let patched = format!("{}{}{}", &patched[..at], MAX_INPUT_BYTES_DOC, &patched[at..]);
    // The runtime __all__ names __version__ too (src/lib.rs), so that
    // maturin's `from .litegen_3d_converter import *` exports it; the
    // generated __all__ leaves dunders out.
    let all = "__all__ = [\n";
    let at = patched.find(all).ok_or("the stub has no __all__")? + all.len();
    let patched = format!("{}    \"__version__\",\n{}", &patched[..at], &patched[at..]);
    std::fs::write(&path, patched)?;
    Ok(())
}
