const REPO = new URL('../../../../../', import.meta.url).pathname; // the litegen checkout
import { readFileSync } from 'node:fs';
globalThis.Image = class { constructor() { this.onload = null; } set src(v) { this._src = v; } };
globalThis.self = globalThis;
const L = REPO + 'dashboard/node_modules/three/examples/jsm/loaders/';
const { USDLoader } = await import(L + 'USDLoader.js');
for (const f of process.argv.slice(2)) {
  const buf = readFileSync(f);
  try {
    const g = new USDLoader().parse(buf.buffer.slice(buf.byteOffset, buf.byteOffset + buf.byteLength));
    const out = [];
    g.traverse(o => {
      if (o.isMesh) {
        const m = Array.isArray(o.material) ? o.material : [o.material];
        const uv = o.geometry.attributes.uv;
        out.push(`${o.isSkinnedMesh ? 'SkinnedMesh' : 'Mesh'} ${o.name} verts=${o.geometry.attributes.position.count} groups=${o.geometry.groups.length} ` +
          `uv0=${uv ? Array.from(uv.array.slice(0, 6)).map(x => +x.toFixed(2)) : '-'} ` +
          `mats=[${m.map(x => `${x.type}(color=${x.color.getHexString()},map=${!!x.map},rough=${x.roughness},metal=${x.metalness},rMap=${!!x.roughnessMap},mMap=${!!x.metalnessMap},nMap=${!!x.normalMap})`).join(' ')}]`);
        if (o.isSkinnedMesh) out.push(`   skeleton bones=${o.skeleton.bones.map(b => b.name)} boneInverse1.t=${o.skeleton.boneInverses[1]?.elements.slice(12, 15)}`);
      }
    });
    console.log('==', f, '\n ', out.join('\n  '));
    for (const c of g.animations) console.log('   clip', c.name, c.tracks.map(t => `${t.name}[${t.times.length}] v1=${Array.from(t.values.slice(t.getValueSize(), 2 * t.getValueSize())).map(x => +x.toFixed(3))}`).join(' | '));
  } catch (e) { console.log('==', f, 'ERROR', e.message); }
}
