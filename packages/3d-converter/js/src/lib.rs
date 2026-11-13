//! The npm package `@litegen/3d-converter`: litegen's 3D converter for
//! Node.js and browsers. A wasm-bindgen layer over `litegen-3d-converter`
//! that decodes JavaScript values into the core's options, calls it, and
//! encodes its results; every rule lives in the core and `litegen-mesh`.

mod api;

use api::{Conversion, ConvertOptions, Format, FormatInfo, InputFileInfo, ModelInfo, ReadOptions, WriteOptions};
use litegen_3d_converter as core;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tsify::{Ts, Tsify};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen(typescript_custom_section)]
const DECLARATIONS: &'static str = r#"
/** One model file's bytes, or several files by name (the model and the files it references, such as an OBJ's `.mtl` and textures). A zip of a package is unpacked. */
export type ModelInput = Uint8Array | Record<string, Uint8Array>;

/** Thrown by every function of this package. */
export declare class ConvertError extends Error {
  /** As the package raises it; a caller seldom builds one. */
  constructor(code: ErrorCode, message: string, param?: string);
  /** Always `"ConvertError"`. */
  readonly name: "ConvertError";
  /** Why the call failed, as litegen's HTTP API spells it. */
  readonly code: ErrorCode;
  /** The option or argument the error is about, in this package's spelling, when there is one. */
  readonly param?: string;
}
"#;

#[wasm_bindgen(module = "/src/convert_error.js")]
extern "C" {
    #[wasm_bindgen(js_name = ConvertError)]
    type JsConvertError;

    #[wasm_bindgen(constructor, js_class = "ConvertError")]
    fn new(code: &str, message: &str, param: Option<String>) -> JsConvertError;
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// Logs a panic's message before wasm aborts, so a bug report has more than
/// "unreachable". The converter's readers return errors for hostile input;
/// a panic is a bug. Installed on first use rather than by a start function,
/// which wasm-bindgen would export.
fn panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::panic::set_hook(Box::new(|info| console_error(&format!("@litegen/3d-converter panicked: {info}")))));
}

fn error(code: &str, message: &str, param: Option<&str>) -> JsValue {
    JsConvertError::new(code, message, param.map(str::to_string)).into()
}

fn core_error(e: core::Error) -> JsValue {
    error(e.code.as_str(), &e.message_with(api::ts_name), e.param.map(api::ts_name))
}

/// The text between `prefix` and the next backtick in `message`.
fn quoted<'m>(message: &'m str, prefix: &str) -> Option<&'m str> {
    let rest = message.strip_prefix(prefix)?;
    rest.split('`').next()
}

/// The field names serde deserializes `T` from, as renamed (`fileName`), read
/// from its own derive: what an options object may contain.
fn fields_of<T: DeserializeOwned>() -> &'static [&'static str] {
    use serde::de::{self, Visitor};
    struct Probe<'a>(&'a mut &'static [&'static str]);
    impl<'de> de::Deserializer<'de> for Probe<'_> {
        type Error = de::value::Error;
        fn deserialize_struct<V: Visitor<'de>>(self, _: &'static str, fields: &'static [&'static str], _: V) -> Result<V::Value, Self::Error> {
            *self.0 = fields;
            Err(de::Error::custom("probed"))
        }
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
            Err(de::Error::custom("not a struct"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
            unit_struct newtype_struct seq tuple tuple_struct map enum identifier ignored_any
        }
    }
    let mut fields: &'static [&'static str] = &[];
    let _ = T::deserialize(Probe(&mut fields));
    fields
}

/// Options from a JavaScript value, failing with the option the value got
/// wrong: an unknown key is `param_unsupported`, a missing required one
/// `param_required`, a bad value `param_invalid`.
fn decode<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    // serde-wasm-bindgen reads only the fields a struct names, so unknown
    // keys are found here, against the struct's own field list.
    if value.is_object() {
        let allowed = fields_of::<T>();
        for key in js_sys::Object::keys(value.unchecked_ref::<js_sys::Object>()).iter() {
            let key = key.as_string().unwrap_or_default();
            if !allowed.contains(&key.as_str()) {
                return Err(error("param_unsupported", &format!("`{key}` is not an option of this function; it takes {}", allowed.join(", ")), Some(&key)));
            }
        }
    }
    let de = serde_wasm_bindgen::Deserializer::from(value);
    serde_path_to_error::deserialize(de).map_err(|e| {
        let path = e.path().to_string();
        let full = e.inner().to_string();
        // serde-wasm-bindgen's errors are JS Errors, displayed "Error: …".
        let message = full.strip_prefix("Error: ").unwrap_or(&full);
        if let Some(name) = quoted(message, "missing field `") {
            return error("param_required", &format!("`{name}` is required"), Some(name));
        }
        match path.split('.').next().filter(|p| !p.is_empty() && *p != "?") {
            Some(field) => error("param_invalid", &format!("`{field}` is invalid: {message}"), Some(field)),
            None => error("param_invalid", &format!("the options are invalid: {message}"), None),
        }
    })
}

fn encode<T: Serialize + Tsify>(value: &T) -> Ts<T>
where
    <T as Tsify>::JsType: JsCast,
{
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    let js = value.serialize(&serializer).expect("the converter's results serialize");
    Ts::new_unchecked(js)
}

/// The input as the core takes it, copied out of JavaScript memory once,
/// and only once the core's cap allows its size.
enum Input {
    Bytes(Vec<u8>),
    Files(Vec<(String, Vec<u8>)>),
}

/// The core's cap, checked on the sizes before anything is copied in:
/// WebAssembly memory only grows, so a refused input must never land in it.
fn check_size(total: usize) -> Result<(), JsValue> {
    core::check_input_size(total).map_err(core_error)
}

impl Input {
    fn decode(value: &JsValue) -> Result<Input, JsValue> {
        if let Some(bytes) = value.dyn_ref::<js_sys::Uint8Array>() {
            check_size(bytes.length() as usize)?;
            return Ok(Input::Bytes(bytes.to_vec()));
        }
        let invalid = || error("param_invalid", "`input` must be a Uint8Array, or an object mapping file names to Uint8Arrays", Some("input"));
        if !value.is_object() || js_sys::Array::is_array(value) {
            return Err(invalid());
        }
        // Bytes in another shape: say how to pass them, rather than read the
        // object's keys as file names.
        let shape = value.unchecked_ref::<js_sys::Object>().constructor().name().as_string().unwrap_or_default();
        let hint = if value.is_instance_of::<js_sys::ArrayBuffer>() {
            Some("pass `new Uint8Array(buffer)`".to_string())
        } else if js_sys::ArrayBuffer::is_view(value) {
            Some("pass its bytes: `new Uint8Array(view.buffer, view.byteOffset, view.byteLength)`".to_string())
        } else if shape == "Blob" || shape == "File" {
            Some("pass `new Uint8Array(await file.arrayBuffer())`".to_string())
        } else {
            None
        };
        if let Some(hint) = hint {
            let article = if shape.starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" };
            return Err(error("param_invalid", &format!("`input` is {article} {shape}, not a Uint8Array; {hint}"), Some("input")));
        }
        let mut arrays = Vec::new();
        for pair in js_sys::Object::entries(value.unchecked_ref()).iter() {
            let pair: js_sys::Array = pair.unchecked_into();
            let name = pair.get(0).as_string().unwrap_or_default();
            let Some(bytes) = pair.get(1).dyn_into::<js_sys::Uint8Array>().ok() else {
                return Err(error("param_invalid", &format!("`input[{name:?}]` must be a Uint8Array"), Some("input")));
            };
            arrays.push((name, bytes));
        }
        check_size(arrays.iter().map(|(_, b)| b.length() as usize).fold(0, usize::saturating_add))?;
        Ok(Input::Files(arrays.into_iter().map(|(name, bytes)| (name, bytes.to_vec())).collect()))
    }

    /// Run `f` on the input as the core takes it, the files borrowed.
    fn with_core<R>(&self, f: impl FnOnce(core::Input<'_>) -> R) -> R {
        match self {
            Input::Bytes(b) => f(core::Input::Bytes(b)),
            Input::Files(files) => {
                let files: Vec<core::InputFile<'_>> = files.iter().map(|(name, data)| core::InputFile { name: name.clone(), data: data.as_slice().into() }).collect();
                f(core::Input::Files(&files))
            }
        }
    }
}

/// Convert a model to another format, in one call.
///
/// `input` is one file's bytes, or several files by name. The result holds
/// the written files (the model first) and every loss.
///
/// @throws {ConvertError}
#[wasm_bindgen]
pub fn convert(#[wasm_bindgen(unchecked_param_type = "ModelInput")] input: JsValue, options: Ts<ConvertOptions>) -> Result<Ts<Conversion>, JsValue> {
    panic_hook();
    let input = Input::decode(&input)?;
    let (read, write) = decode::<ConvertOptions>(options.js_value())?.split();
    let converted = input.with_core(|input| core::convert(input, &read, &write)).map_err(core_error)?;
    Ok(encode(&Conversion::from(converted)))
}

/// Read a model once, to inspect it and to write it to as many formats as
/// needed. Call `free()` (or `using`) when done: the model lives in
/// WebAssembly memory.
///
/// @throws {ConvertError}
#[wasm_bindgen(js_name = readModel)]
pub fn read_model(#[wasm_bindgen(unchecked_param_type = "ModelInput")] input: JsValue, options: Option<Ts<ReadOptions>>) -> Result<Model, JsValue> {
    panic_hook();
    let input = Input::decode(&input)?;
    let options: ReadOptions = match options {
        Some(o) => decode(o.js_value())?,
        None => ReadOptions::default(),
    };
    let options = options.into();
    let inner = input.with_core(|input| core::Model::read(input, &options)).map_err(core_error)?;
    Ok(Model { inner })
}

/// The format of a file, from its bytes and, when they cannot tell, its
/// name. `undefined` when neither says.
///
/// @throws {ConvertError} `input_too_large` past `maxInputBytes()`, before
/// the bytes are copied into WebAssembly memory.
#[wasm_bindgen(js_name = detectFormat)]
#[allow(non_snake_case)] // `fileName`: wasm-bindgen names TypeScript parameters after Rust's
pub fn detect_format(input: js_sys::Uint8Array, fileName: Option<String>) -> Result<Option<Ts<Format>>, JsValue> {
    panic_hook();
    check_size(input.length() as usize)?;
    Ok(core::detect_format(&input.to_vec(), fileName.as_deref()).map(|f| encode(&Format::from(f))))
}

/// Every format, what it keeps and which options it takes.
#[wasm_bindgen]
pub fn formats() -> Vec<Ts<FormatInfo>> {
    core::formats().into_iter().map(|i| encode(&FormatInfo::from(i))).collect()
}

/// The largest input, in bytes, the converter reads.
#[wasm_bindgen(js_name = maxInputBytes)]
pub fn max_input_bytes() -> u32 {
    u32::try_from(core::MAX_INPUT_BYTES).unwrap_or(u32::MAX)
}

/// This package's version.
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// A model read once, ready to write to any number of formats.
#[wasm_bindgen]
pub struct Model {
    inner: core::Model,
}

#[wasm_bindgen]
impl Model {
    /// The format it was read as.
    #[wasm_bindgen(getter)]
    pub fn format(&self) -> Ts<Format> {
        encode(&Format::from(self.inner.format()))
    }

    /// What the reader measures in it: counts, bounds, materials, skin and
    /// clips. Measured on first use (reading and writing never measure),
    /// and placing every instance of every mesh to do it.
    ///
    /// @throws {ConvertError} `mesh_unsupported` for a model past 4,000,000
    /// vertices once every instance is placed, which still converts.
    #[wasm_bindgen(getter)]
    pub fn info(&self) -> Result<Ts<ModelInfo>, JsValue> {
        Ok(encode(&ModelInfo::from(self.inner.info().map_err(core_error)?)))
    }

    /// What reading it dropped or approximated, in prose.
    #[wasm_bindgen(getter)]
    pub fn warnings(&self) -> Vec<String> {
        self.inner.warnings().to_vec()
    }

    /// Every input file and the role it played.
    #[wasm_bindgen(getter)]
    pub fn files(&self) -> Vec<Ts<InputFileInfo>> {
        self.inner.files().iter().map(|f| encode(&InputFileInfo::from(f))).collect()
    }

    /// Write the model as `options` says.
    ///
    /// @throws {ConvertError}
    pub fn write(&self, options: Ts<WriteOptions>) -> Result<Ts<Conversion>, JsValue> {
        let options: WriteOptions = decode(options.js_value())?;
        let written = self.inner.write(&options.into()).map_err(core_error)?;
        Ok(encode(&Conversion::from(written)))
    }
}
