// The error every function of @litegen/3d-converter throws. It lives in JS so
// it is a real Error subclass (`instanceof ConvertError`, a stack trace); the
// Rust side constructs it through wasm-bindgen. Its TypeScript declaration is
// in src/lib.rs's typescript_custom_section.
export class ConvertError extends Error {
  constructor(code, message, param) {
    super(message);
    this.name = 'ConvertError';
    this.code = code;
    if (param !== undefined) this.param = param;
  }
}
