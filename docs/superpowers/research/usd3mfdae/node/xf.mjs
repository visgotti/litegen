const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
import { readFileSync } from 'node:fs';
const L = REPO + 'dashboard/node_modules/three/examples/jsm/loaders/';
const { USDLoader } = await import(L + 'USDLoader.js');
for (const f of process.argv.slice(2)) {
  const b = readFileSync(f);
  const g = new USDLoader().parse(f.endsWith('.usda') ? b.toString() : b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength));
  g.traverse(o => { if (o.isMesh) console.log(f, o.name, 'position=', o.position.toArray()); });
}
