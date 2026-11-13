// @litegen/3d-converter's browser entry. The wasm is fetched and instantiated
// before this module resolves (top-level await), so every function is ready
// on import. Vite and webpack 5 emit the wasm as an asset from this
// `new URL(…, import.meta.url)`; with other bundlers, copy it beside the
// bundle (README.md, "Bundlers and workers").
import init from './litegen_3d_converter.js';
import './model.js';

await init({ module_or_path: new URL('./litegen_3d_converter_bg.wasm', import.meta.url) });

export * from './litegen_3d_converter.js';
export { ConvertError } from '__CONVERT_ERROR__';
