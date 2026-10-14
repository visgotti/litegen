const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
import { readFileSync } from 'node:fs';
const base = REPO + 'dashboard/node_modules/three/examples/jsm/loaders/usd/';
const { USDCParser } = await import(base + 'USDCParser.js');
const { USDAParser } = await import(base + 'USDAParser.js');
const [,, file, ...paths] = process.argv;
const buf = readFileSync(file);
let data;
if (buf.subarray(0, 8).toString() === 'PXR-USDC') {
  const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
  data = new USDCParser().parseData(ab);
} else {
  data = new USDAParser().parseData(buf.toString('utf8'));
}
for (const p of paths) {
  const s = data.specsByPath[p];
  console.log(p, '=>', s ? JSON.stringify(s.fields, (k, v) => (ArrayBuffer.isView(v) ? Array.from(v) : v)) : 'MISSING');
}
