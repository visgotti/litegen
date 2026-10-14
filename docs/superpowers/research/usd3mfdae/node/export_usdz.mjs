const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
import { writeFileSync } from 'node:fs';
const T = REPO + 'dashboard/node_modules/three/';
const THREE = await import(T + 'build/three.module.js');
const { USDZExporter } = await import(T + 'examples/jsm/exporters/USDZExporter.js');
const scene = new THREE.Scene();
const g = new THREE.Group(); g.name = 'Grp'; g.position.set(1, 0, 0); scene.add(g);
const m = new THREE.Mesh(new THREE.BoxGeometry(1, 1, 1), new THREE.MeshStandardMaterial({ color: 0x3366ff, roughness: 0.4, metalness: 0.1 }));
m.name = 'Box'; g.add(m);
const out = await new USDZExporter().parseAsync(scene, { quickLookCompatible: true });
writeFileSync(process.argv[2], out);
console.log('wrote', out.byteLength);
