// The package as Node imports it (through its own exports map), on files
// litegen's writer and independent tools wrote.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  convert, readModel, detectFormat, formats, maxInputBytes, version, ConvertError, Model,
} from '@litegen/3d-converter';
import { instancedGlb } from './glb.mjs';

const testdata = (path) =>
  new Uint8Array(readFileSync(new URL(`../../../../litegen-core/src/mesh/testdata/${path}`, import.meta.url)));
const text = (bytes) => new TextDecoder().decode(bytes);
const starts = (bytes, ascii) => text(bytes.subarray(0, ascii.length)) === ascii;

const SKINNED_GLB = testdata('fbx/skinned_small_source.glb');
// Seven joints named as litegen's mock provider names a humanoid's.
const HUMANOID_MOCK = new Uint8Array(readFileSync(new URL('../../testdata/humanoid_mock.gltf', import.meta.url)));

const MEDIA = {
  glb: 'model/gltf-binary', gltf: 'model/gltf+json', obj: 'model/obj', stl: 'model/stl',
  ply: 'application/octet-stream', '3mf': 'model/3mf', dae: 'model/vnd.collada+xml',
  fbx: 'application/octet-stream', usdz: 'model/vnd.usdz+zip', usda: 'model/vnd.usda',
  usdc: 'application/octet-stream',
};

const LOOKS = {
  glb: (d) => starts(d, 'glTF'),
  gltf: (d) => JSON.parse(text(d)).asset.version === '2.0',
  obj: (d) => text(d).includes('\nv '),
  stl: (d) => d.length === 84 + 50 * new DataView(d.buffer, d.byteOffset).getUint32(80, true),
  ply: (d) => starts(d, 'ply'),
  '3mf': (d) => starts(d, 'PK\x03\x04'),
  dae: (d) => text(d).includes('<COLLADA'),
  fbx: (d) => starts(d, 'Kaydara FBX Binary'),
  usdz: (d) => starts(d, 'PK\x03\x04'),
  usda: (d) => starts(d, '#usda'),
  usdc: (d) => starts(d, 'PXR-USDC'),
};

test('converts glb to every format', () => {
  for (const to of Object.keys(LOOKS)) {
    const c = convert(SKINNED_GLB, { to });
    assert.equal(c.format, to);
    assert.equal(c.files.length, 1, to);
    const [file] = c.files;
    assert.ok(file.data instanceof Uint8Array, to);
    assert.ok(LOOKS[to](file.data), `${to} output looks like ${to}`);
    assert.equal(file.mediaType, MEDIA[to], to);
    assert.equal(file.role, 'model');
    assert.equal(file.name, `model.${to}`);
  }
});

test('errors are ConvertError, with the API codes and the option named in TS spelling', () => {
  const cases = [
    [() => convert(SKINNED_GLB, { from: 'fbx', to: 'obj' }), 'format_mismatch', 'from'],
    [() => convert(SKINNED_GLB, { to: 'glb', units: 'mm' }), 'param_unsupported', 'units'],
    [() => convert(SKINNED_GLB, { to: 'stl', fromUnits: 'mm' }), 'param_unsupported', 'fromUnits'],
    [() => convert(SKINNED_GLB, { to: 'obj', fbxLightUnits: 'blender' }), 'param_unsupported', 'fbxLightUnits'],
    [() => convert(SKINNED_GLB, { to: 'fbx', usdLightUnits: 'blender' }), 'param_unsupported', 'usdLightUnits'],
    [() => convert(SKINNED_GLB, { to: 'fbx', fbxLightUnits: 'watts' }), 'param_invalid', 'fbxLightUnits'],
    [() => convert(SKINNED_GLB, { to: 'glb', daeMorphs: 'keep' }), 'param_unsupported', 'daeMorphs'],
    [() => convert(SKINNED_GLB, { to: 'dae', daeMorphs: 'baked' }), 'param_invalid', 'daeMorphs'],
    [() => convert(SKINNED_GLB, { to: 'usda', usdzRoot: 'usdc' }), 'param_unsupported', 'usdzRoot'],
    [() => convert(SKINNED_GLB, { to: 'usdz', usdzRoot: 'usd' }), 'param_invalid', 'usdzRoot'],
    [() => convert(SKINNED_GLB, { to: 'glb', usdClips: 'all' }), 'param_unsupported', 'usdClips'],
    [() => convert(SKINNED_GLB, { to: 'usdc', usdClips: 'none' }), 'param_invalid', 'usdClips'],
    [() => convert(SKINNED_GLB, { to: 'xyz' }), 'param_invalid', 'to'],
    [() => convert(SKINNED_GLB, { to: 'glb', scale: 2 }), 'param_unsupported', 'scale'],
    [() => convert(SKINNED_GLB, {}), 'param_required', 'to'],
    [() => convert('not bytes', { to: 'glb' }), 'param_invalid', 'input'],
    [() => convert(new TextEncoder().encode('hello'), { to: 'glb' }), 'format_unrecognized', undefined],
  ];
  for (const [call, code, param] of cases) {
    assert.throws(call, (e) => {
      assert.ok(e instanceof ConvertError, `${code}: ${e}`);
      assert.ok(e instanceof Error);
      assert.equal(e.name, 'ConvertError');
      assert.equal(e.code, code, e.message);
      assert.equal(e.param, param, e.message);
      assert.ok(e.message.length > 0);
      return true;
    });
  }
  assert.throws(() => convert(SKINNED_GLB, { to: 'glb', units: 'mm' }), {
    message: '`units` applies when `to` is stl, obj or ply, which carry no units or up axis; glb declares its own',
  });
  assert.throws(() => convert(SKINNED_GLB, { to: 'glb', daeMorphs: 'keep' }), {
    message: '`daeMorphs` applies when `to` is dae',
  });
  assert.throws(() => convert(SKINNED_GLB, { to: 'usda', usdzRoot: 'usdc' }), {
    message: '`usdzRoot` applies when `to` is usdz',
  });
  assert.throws(() => convert(SKINNED_GLB, { to: 'glb', usdClips: 'all' }), {
    message: '`usdClips` applies when `to` is usda, usdc or usdz',
  });
});

test('usdc writes a crate, which reads back under either USD name', () => {
  const usdc = convert(SKINNED_GLB, { to: 'usdc' }).files[0];
  assert.deepEqual([usdc.name, usdc.mediaType], ['model.usdc', 'application/octet-stream']);
  assert.equal(detectFormat(usdc.data), 'usdc');
  const asUsdc = convert(usdc.data, { from: 'usdc', to: 'glb' }).files[0].data;
  assert.deepEqual(convert(usdc.data, { from: 'usda', to: 'glb' }).files[0].data, asUsdc);
});

test('usdzRoot picks the USDZ root layer, and usdClips reaches every USD write', () => {
  // A zip's first entry name: the local file header's name length at 26,
  // the name at 30.
  const root = (data) => text(data.subarray(30, 30 + new DataView(data.buffer, data.byteOffset).getUint16(26, true)));
  assert.equal(root(convert(SKINNED_GLB, { to: 'usdz' }).files[0].data), 'model.usda');
  assert.equal(root(convert(SKINNED_GLB, { to: 'usdz', usdzRoot: 'usda' }).files[0].data), 'model.usda');
  assert.equal(root(convert(SKINNED_GLB, { to: 'usdz', usdzRoot: 'usdc' }).files[0].data), 'model.usdc');
  for (const to of ['usda', 'usdc', 'usdz']) {
    for (const usdClips of ['all', 'first']) {
      const m = readModel(SKINNED_GLB);
      try {
        assert.deepEqual(m.write({ to, usdClips }), convert(SKINNED_GLB, { to, usdClips }), `${to}, ${usdClips}`);
      } finally {
        m.free();
      }
    }
  }
});

test('daeMorphs bakes or keeps the morph targets', () => {
  const morph = testdata('gltf/gltfpack/morph.glb');
  const bake = convert(morph, { to: 'dae' });
  const keep = convert(morph, { to: 'dae', daeMorphs: 'keep' });
  assert.deepEqual(convert(morph, { to: 'dae', daeMorphs: 'bake' }), bake);
  assert.notDeepEqual(keep.files[0].data, bake.files[0].data);
  assert.ok(bake.losses.some((l) => l.kind === 'morph_targets'), JSON.stringify(bake.losses));
  assert.ok(!keep.losses.some((l) => l.kind === 'morph_targets'), JSON.stringify(keep.losses));
  const m = readModel(morph);
  try {
    assert.deepEqual(m.write({ to: 'dae', daeMorphs: 'keep' }), keep);
  } finally {
    m.free();
  }
});

test('a model written many times equals each one-shot convert', () => {
  const m = readModel(SKINNED_GLB);
  try {
    assert.ok(m instanceof Model);
    assert.equal(m.format, 'glb');
    for (const to of ['fbx', 'usdz', 'obj', 'stl']) {
      assert.deepEqual(m.write({ to }), convert(SKINNED_GLB, { to }), to);
    }
  } finally {
    m.free();
  }
});

const FREED = { name: 'TypeError', message: 'this Model was freed; read the model again with readModel()' };

test('write after free throws, and the module still works', () => {
  const m = readModel(SKINNED_GLB);
  m.free();
  assert.throws(() => m.write({ to: 'obj' }), FREED);
  assert.equal(readModel(SKINNED_GLB).format, 'glb');
});

test('disposes with using', () => {
  let held;
  {
    using m = readModel(SKINNED_GLB);
    held = m;
    assert.equal(m.format, 'glb');
  }
  assert.throws(() => held.write({ to: 'obj' }), FREED);
});

test('model facts and file roles', () => {
  const m = readModel(SKINNED_GLB);
  assert.deepEqual(m.info, {
    triangles: 48, vertices: 35, nodes: 4, meshes: 1, textures: 1, uvSets: 1, vertexColors: false,
    bounds: { min: [-0.30000001192092896, 0, -0.30000001192092896], max: [0.30000001192092896, 3, 0.30000001192092896] },
    materials: [{ name: 'tube_mat', maps: ['base_color'] }],
    skin: { joints: 3, root: 'hip', bindPose: 'unknown' },
    animations: [{ name: 'Bend', duration: 2, rootTravel: [0, 0, 0], inPlace: true }],
  });
  // Values the first file leaves at their defaults.
  using travelling = readModel(testdata('gltf/gltfpack/skinned.glb'));
  assert.deepEqual(travelling.info.animations, [{ name: 'Bend', duration: 2, rootTravel: [0.4000000059604645, 0, 0], inPlace: false }]);
  using coloured = readModel(testdata('fbx/textured_assimp_7500.fbx'));
  assert.equal(coloured.info.vertexColors, true);
  using multiuv = readModel(testdata('gltf/gltfpack/multiuv.glb'));
  assert.equal(multiuv.info.uvSets, 4);
  assert.deepEqual(m.files, [{ name: 'model.glb', role: 'model', size: SKINNED_GLB.length }]);
  m.free();
});

test('several files read as one model, with each file in its role', () => {
  const pkg = convert(testdata('fbx/textured_assimp_7500.fbx'), { to: 'obj', package: 'files' });
  assert.ok(pkg.files.length >= 3, 'obj, mtl and a texture');
  const roles = Object.fromEntries(pkg.files.map((f) => [f.name, f.role]));
  assert.ok(Object.values(roles).includes('material_library'));
  assert.ok(Object.values(roles).includes('texture'));
  const record = Object.fromEntries(pkg.files.map((f) => [f.name, f.data]));
  const m = readModel(record);
  assert.equal(m.format, 'obj');
  assert.ok(m.info.textures >= 1);
  assert.deepEqual(Object.fromEntries(m.files.map((f) => [f.name, f.role])), roles);
  m.free();
});

test('a package zip reads back', () => {
  const zipped = convert(testdata('fbx/textured_assimp_7500.fbx'), { to: 'obj', package: 'zip' });
  assert.equal(zipped.files[0].name, 'model.obj.zip');
  assert.equal(zipped.files[0].mediaType, 'application/zip');
  const m = readModel(zipped.files[0].data);
  assert.equal(m.format, 'obj');
  assert.ok(m.info.textures >= 1);
  m.free();
});

test('detectFormat', () => {
  const cases = {
    'fbx/textured_assimp_7500.fbx': 'fbx',
    'fbx/cube_assimp_ascii.fbx': 'fbx',
    'dae/scenekit/skinned_scenekit.dae': 'dae',
    '3mf/assimp_textured.3mf': '3mf',
    'usd/textured_apple.usdz': 'usdz',
    'usd/cube.usdc': 'usdc',
    'usd/comp.usda': 'usda',
    'ply/cube_be_struct.ply': 'ply',
    'fbx/skinned_small_source.glb': 'glb',
  };
  for (const [path, want] of Object.entries(cases)) {
    assert.equal(detectFormat(testdata(path)), want, path);
  }
  assert.equal(detectFormat(new Uint8Array(0), 'scene.usdc'), 'usdc');
  assert.equal(detectFormat(new Uint8Array(0), 'scene.usd'), 'usda');
  assert.equal(detectFormat(testdata('usd/cube.usdc'), 'scene.usd'), 'usdc');
  assert.equal(detectFormat(new TextEncoder().encode('hello')), undefined);
});

test('formats, limits and version', () => {
  const all = formats();
  assert.deepEqual(all.map((f) => f.format), ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx', 'usdz', 'usda', 'usdc']);
  const stl = all.find((f) => f.format === 'stl');
  assert.deepEqual(stl.readOptions, ['fromUnits', 'fromUp']);
  assert.deepEqual(stl.writeOptions, ['units', 'up']);
  const fbx = all.find((f) => f.format === 'fbx');
  assert.deepEqual(fbx.readOptions, ['fbxColorSpace', 'fbxLightUnits']);
  assert.deepEqual(fbx.writeOptions, ['fbxColorSpace', 'fbxLightUnits', 'fps']);
  const usdz = all.find((f) => f.format === 'usdz');
  assert.deepEqual([usdz.readOptions, usdz.writeOptions], [['usdLightUnits'], ['usdLightUnits', 'usdzRoot', 'usdClips', 'fps']]);
  for (const name of ['usda', 'usdc']) {
    const usd = all.find((f) => f.format === name);
    assert.deepEqual([usd.readOptions, usd.writeOptions, usd.extensions], [['usdLightUnits'], ['usdLightUnits', 'usdClips', 'fps'], [name, 'usd']]);
  }
  const dae = all.find((f) => f.format === 'dae');
  assert.deepEqual([dae.readOptions, dae.writeOptions], [[], ['daeMorphs', 'fps']]);
  assert.equal(stl.storedFrame, 'caller');
  assert.equal(all.find((f) => f.format === '3mf').storedFrame, 'z_up');
  assert.equal(all.find((f) => f.format === 'obj').keepsInPackage.materials, 'phong');
  assert.equal(maxInputBytes(), 134217728);
  assert.match(version(), /^\d+\.\d+\.\d+$/);
});

test('a lone zip may carry its own name', () => {
  const zip = convert(testdata('fbx/textured_assimp_7500.fbx'), { to: 'obj', package: 'zip' }).files[0].data;
  using named = readModel(zip, { fileName: 'fox.zip' });
  assert.equal(named.format, 'obj');
  assert.equal(convert({ 'fox.zip': zip }, { to: 'glb', fileName: 'fox.zip' }).format, 'glb');
  // A name inside the zip still says which file is the model.
  assert.equal(convert({ 'fox.zip': zip }, { to: 'glb', fileName: 'model.obj' }).format, 'glb');
  assert.throws(() => readModel({ 'fox.zip': zip }, { fileName: 'other.zip' }), (e) => e.code === 'param_invalid' && e.param === 'fileName');
});

test('an OBJ whose first face is past the first 64 KiB is still detected', () => {
  let obj = '';
  for (let i = 0; i < 4000; i++) obj += `v ${i}.000000 0.000000 0.000000\n`;
  obj += 'f 1 2 3\n';
  const bytes = new TextEncoder().encode(obj);
  assert.ok(bytes.length > 64 * 1024);
  assert.equal(detectFormat(bytes), 'obj');
  assert.equal(convert(bytes, { to: 'stl' }).format, 'stl');
});

test('reading and converting never measure the model; info does, and may refuse', () => {
  // 70 nodes place one 60,000-vertex mesh: 4.2 million vertices once every
  // instance is placed, past what measuring flattens (4 million). The hosted
  // converter, which never measures, converts it.
  const glb = instancedGlb(70);
  assert.equal(convert(glb, { to: 'glb' }).format, 'glb');
  using model = readModel(glb);
  assert.equal(model.write({ to: 'gltf' }).format, 'gltf');
  assert.throws(() => model.info, (e) => e instanceof ConvertError && e.code === 'mesh_unsupported');
  using small = readModel(instancedGlb(2, 10));
  assert.equal(small.info.triangles, 10, 'each mesh counted once, however many nodes place it');
});

test('materials name their maps and a humanoid rig its bones', () => {
  const MAPS = ['base_color', 'metallic_roughness', 'normal', 'occlusion', 'emissive', 'clearcoat', 'clearcoat_roughness',
    'clearcoat_normal', 'transmission', 'volume_thickness', 'specular', 'specular_color', 'sheen_color', 'sheen_roughness',
    'iridescence', 'iridescence_thickness', 'anisotropy'];
  using textured = readModel(testdata('fbx/textured_assimp_7500.fbx'));
  const maps = textured.info.materials.flatMap((m) => m.maps);
  assert.ok(maps.includes('base_color'), JSON.stringify(maps));
  assert.ok(maps.every((m) => MAPS.includes(m)), JSON.stringify(maps));
  using rig = readModel(HUMANOID_MOCK);
  assert.deepEqual(rig.info.skin.humanoid, {
    naming: 'litegen-mock',
    bones: [
      { bone: 'hips', joint: 'hips' }, { bone: 'spine', joint: 'torso' }, { bone: 'head', joint: 'head' },
      { bone: 'leftUpperArm', joint: 'arm_l' }, { bone: 'rightUpperArm', joint: 'arm_r' },
      { bone: 'leftUpperLeg', joint: 'leg_l' }, { bone: 'rightUpperLeg', joint: 'leg_r' },
    ],
  });
  using plain = readModel(SKINNED_GLB);
  assert.equal(plain.info.skin.humanoid, undefined, 'no naming scheme matches its three joints');
});

test('free is safe to repeat, and a freed model says it was freed', () => {
  const m = readModel(SKINNED_GLB);
  m.free();
  m.free();
  m[Symbol.dispose]();
  assert.throws(() => m.write({ to: 'obj' }), { name: 'TypeError', message: /freed/ });
  assert.throws(() => m.info, { name: 'TypeError', message: /freed/ });
  {
    using inner = readModel(SKINNED_GLB);
    inner.free(); // then `using` disposes it again at the block's end
  }
  assert.equal(readModel(SKINNED_GLB).format, 'glb');
});

test('version() is the package version', () => {
  const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
  assert.equal(version(), pkg.version);
});

test('bytes in another shape say how to pass them', () => {
  const bytes = new Uint8Array([1, 2, 3]);
  const view = 'pass its bytes: `new Uint8Array(view.buffer, view.byteOffset, view.byteLength)`';
  const shapes = [
    [bytes.buffer, '`input` is an ArrayBuffer, not a Uint8Array; pass `new Uint8Array(buffer)`'],
    [new DataView(bytes.buffer), `\`input\` is a DataView, not a Uint8Array; ${view}`],
    [new Int8Array(2_000_000), `\`input\` is an Int8Array, not a Uint8Array; ${view}`],
    [new Blob([bytes]), '`input` is a Blob, not a Uint8Array; pass `new Uint8Array(await file.arrayBuffer())`'],
  ];
  for (const [input, message] of shapes) {
    const started = performance.now();
    assert.throws(() => convert(input, { to: 'glb' }), { name: 'ConvertError', code: 'param_invalid', param: 'input', message });
    assert.ok(performance.now() - started < 50, `${input.constructor.name} answered at once`);
  }
});
