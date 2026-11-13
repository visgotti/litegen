// @litegen/3d-converter's Node.js entry. The wasm is instantiated
// synchronously from disk, so importing this module needs no top-level await
// and every function is ready on import.
import { readFileSync } from 'node:fs';
import { initSync } from './litegen_3d_converter.js';
import './model.js';

initSync({ module: readFileSync(new URL('./litegen_3d_converter_bg.wasm', import.meta.url)) });

export * from './litegen_3d_converter.js';
export { ConvertError } from '__CONVERT_ERROR__';
