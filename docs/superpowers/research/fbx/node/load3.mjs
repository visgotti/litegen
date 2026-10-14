const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
// Load FBX files with three.js FBXLoader (read-only use of the repo's node_modules copy).
// Usage: node load3.mjs FILE...
import { readFileSync } from 'node:fs';
const base = REPO + 'dashboard/node_modules/three/examples/jsm/loaders/FBXLoader.js';
const { FBXLoader } = await import(base);
const origWarn = console.warn; const warns = [];
console.warn = (...a) => warns.push(a.join(' ').slice(0, 160));
for (const f of process.argv.slice(2)) {
  warns.length = 0;
  const buf = readFileSync(f);
  const ab = buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength);
  let line = f.split('/').pop().padEnd(26);
  try {
    const g = new FBXLoader().parse(ab, '');
    const meshes = []; let bones = 0;
    g.traverse(o => { if (o.isMesh) meshes.push(o); if (o.isBone) bones++; });
    const m = meshes[0];
    line += ` meshes=${meshes.length}`;
    if (m) {
      const pos = m.geometry.attributes.position;
      const uv = m.geometry.attributes.uv;
      const n = m.geometry.attributes.normal;
      line += ` tris=${pos.count / 3} uv0=${uv ? Array.from(uv.array.slice(0, 4)).map(x => +x.toFixed(2)) : 'none'} n0=${n ? Array.from(n.array.slice(0, 3)).map(x => +x.toFixed(2)) : 'none'}`;
      line += ` skinned=${!!m.isSkinnedMesh} mat=${m.material.name}:${m.material.color ? m.material.color.getHexString() : '-'}`;
      if (m.isSkinnedMesh) line += ` bindMatrixIdentity=${m.bindMatrix.equals(new m.bindMatrix.constructor())}`;
    }
    line += ` bones=${bones} clips=${g.animations.length}`;
    for (const c of g.animations) line += ` [${c.name}: ${c.tracks.map(t => t.name + '(' + t.times.length + ')').join(',')} dur=${c.duration.toFixed(2)}]`;
    if (g.userData.unitScaleFactor !== undefined) line += ` unitScaleFactor=${g.userData.unitScaleFactor}`;
  } catch (e) {
    line += ' THROW: ' + String(e.message || e).slice(0, 150);
  }
  console.log(line + (warns.length ? '  WARN: ' + [...new Set(warns)].join(' | ') : ''));
}
