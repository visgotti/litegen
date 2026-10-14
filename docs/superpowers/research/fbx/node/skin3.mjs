const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
// Pose a loaded FBX at t=1s and print skinned world positions of vertices (three.js FBXLoader).
import { readFileSync } from 'node:fs';
const { FBXLoader } = await import(REPO + 'dashboard/node_modules/three/examples/jsm/loaders/FBXLoader.js');
const THREE = await import(REPO + 'dashboard/node_modules/three/build/three.module.js');
for (const f of process.argv.slice(2)) {
  const b = readFileSync(f);
  const g = new FBXLoader().parse(b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength), '');
  const mixer = new THREE.AnimationMixer(g);
  mixer.clipAction(g.animations[0]).play();
  mixer.setTime(0.5);
  g.updateMatrixWorld(true);
  let mesh; g.traverse(o => { if (o.isSkinnedMesh) mesh = o; });
  const seen = new Map(); const v = new THREE.Vector3();
  for (let i = 0; i < mesh.geometry.attributes.position.count; i++) {
    mesh.getVertexPosition(i, v); const key = Array.from(mesh.geometry.attributes.position.array.slice(i*3,i*3+3)).join(",");
    v.applyMatrix4(mesh.matrixWorld); // getVertexPosition already applies skinning on SkinnedMesh
    seen.set(key, v.toArray().map(x => +x.toFixed(2)).join(','));
  }
  console.log("pos=", mesh.position.toArray(), "bind=", mesh.bindMatrix.elements.slice(12,15), "raw0=", Array.from(mesh.geometry.attributes.position.array.slice(0,3)));
  console.log(f.split('/').pop(), [...seen.entries()].map(([k, w]) => `(${k})->(${w})`).join(' '));
}
