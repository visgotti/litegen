// Writes public/samples/: one sample model per source format, each a
// different kind of model, and samples.json describing them. Run with
// `npm run samples` after building the package.
//
// Every file is litegen's own content (mesh/testdata's READMEs): some as
// independent tools wrote them (assimp, SceneKit, Apple's USD tools), the
// multi-file ones converted by @litegen/3d-converter itself.
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { convert, readModel } from '@litegen/3d-converter';

const here = path.dirname(fileURLToPath(import.meta.url));
const testdata = path.resolve(here, '../../../litegen-core/src/mesh/testdata');
const out = path.resolve(here, '../public/samples');
const read = (p) => new Uint8Array(readFileSync(path.join(testdata, p)));

/** id, title, description, how to make its files, and extras: `reference`
 *  names the conversion the e2e matrix compares shapes with when the
 *  format's three.js loader draws the input incompletely. */
const SAMPLES = [
  ['skinned-glb', 'Skinned, animated', "litegen's skinned test character, as glTF Binary.", () => ({ 'skinned.glb': read('fbx/skinned_small_source.glb') })],
  ['textured-gltf', 'Textured, three files', "Apple's textured USDZ, as glTF with its .bin and PNG beside it.", () => files(convert(read('usd/textured_apple.usdz'), { to: 'gltf', package: 'files' }))],
  ['textured-obj', 'Textured, OBJ + MTL + PNG', "assimp's textured FBX, as OBJ with its material library and texture.", () => files(convert(read('fbx/textured_assimp_7500.fbx'), { to: 'obj', package: 'files' }))],
  ['cube-stl', 'Geometry only', 'A cube as binary STL: triangles, nothing else.', () => files(convert(read('fbx/cube_assimp_ascii.fbx'), { to: 'stl' }))],
  ['cube-ply', 'Big-endian PLY', 'A cube as a big-endian binary PLY.', () => ({ 'cube_be.ply': read('ply/cube_be_struct.ply') })],
  // assimp wrote this 3MF from a Y-up glTF without turning it, and in metres
  // labelled as millimetres; the converter reads what the file says.
  ['textured-3mf', 'Coloured 3MF', 'A model with base colours that assimp wrote as 3MF. Its numbers are Y up and labelled millimetres, so it reads as a 1.6 mm model on its back, as the file says.', () => ({ 'textured.3mf': read('3mf/assimp_textured.3mf') })],
  // SceneKit's file names textures/bands.png, the skinned character's own
  // texture, which mesh's test data doesn't ship beside it: it comes from
  // the package's own Collada package of that character.
  ['skinned-dae', 'Skinned COLLADA', "A skinned model written by Apple's SceneKit, with its texture.", () => ({
    'skinned.dae': read('dae/scenekit/skinned_scenekit.dae'),
    'textures/bands.png': convert(read('fbx/skinned_small_source.glb'), { to: 'dae', package: 'files' }).files.find((f) => f.name === 'textures/bands.png').data,
  })],
  ['textured-fbx', 'Textured FBX', 'A textured model written by assimp as binary FBX 7500.', () => ({ 'textured.fbx': read('fbx/textured_assimp_7500.fbx') })],
  // three.js's USDZLoader draws three of this file's four meshes, untextured
  // and in displayColor rather than the bound materials, so the e2e matrix
  // compares its conversions with its glb conversion, not with that view.
  ['textured-usdz', 'Textured USDZ', "A textured model written by Apple's USD tools.", () => ({ 'textured.usdz': read('usd/textured_apple.usdz') }), { reference: 'glb' }],
  ['textured-usda', 'Textured, USD text + PNG', "assimp's textured FBX, as a USD text layer with its texture beside it.", () => files(convert(read('fbx/textured_assimp_7500.fbx'), { to: 'usda', package: 'files' }))],
];

function files(conversion) {
  return Object.fromEntries(conversion.files.map((f) => [f.name, f.data]));
}

rmSync(out, { recursive: true, force: true });
const manifest = [];
for (const [id, title, description, make, extra = {}] of SAMPLES) {
  const set = make();
  const model = readModel(set);
  const entry = {
    id,
    title,
    description,
    format: model.format,
    static: model.info.animations.length === 0,
    files: Object.keys(set),
    ...extra,
  };
  model.free();
  for (const [name, data] of Object.entries(set)) {
    const target = path.join(out, id, name);
    mkdirSync(path.dirname(target), { recursive: true });
    writeFileSync(target, data);
  }
  manifest.push(entry);
}
const formats = manifest.map((s) => s.format);
const missing = ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx', 'usdz', 'usda'].filter((f) => !formats.includes(f));
if (missing.length > 0 || new Set(formats).size !== formats.length) throw new Error(`one sample per format: got ${formats.join(', ')}`);
writeFileSync(path.join(out, 'samples.json'), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(manifest.map((s) => `${s.format.padEnd(5)} ${s.id.padEnd(14)} static=${s.static} files=${s.files.join(',')}`).join('\n'));
