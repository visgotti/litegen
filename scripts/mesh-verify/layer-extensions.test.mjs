// Tests for layer-extensions.mjs, the semantic comparison of a glTF source's
// material layers with litegen's output (Ruling A20). Run, from the repo:
//
//   node --test scripts/mesh-verify/layer-extensions.test.mjs
//
// One test reads Khronos's nine schema files from the folder KHRONOS_SCHEMAS
// names and is skipped without it (README, "The layer-extension check").
//
// Ruling IDs (A20, A20a) are mesh-gaps item 4's rulings, listed with their
// costs in the status block of
// docs/superpowers/plans/2026-11-04-mesh-material-layers.md.
//
// Every expectation below is a hand-written literal. The fixtures are tiny
// glTF/GLB files written by the helpers at the bottom; their images are
// arbitrary bytes (only their sha-256 matters to the check).
import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { compareFiles, DEFAULTS } from './layer-extensions.mjs';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'layer-extensions-test-'));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
let n = 0;

// ─── the verdict and the defaults ───────────────────────────────────────────

test('equal layers pass, and each material is paired with its layers listed', () => {
  const m = [{ name: 'coated', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0.5 } } }];
  const r = compareFiles(gltf({ materials: m }), gltf({ materials: m }));
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.diffs, []);
  assert.equal(r.materials.length, 1);
  assert.deepEqual(r.materials[0].layers, { src: ['clearcoat'], out: ['clearcoat'] });
  assert.equal(r.materials[0].out, 0);
});

test('an absent field equals its schema default, for every one of the nine layers', () => {
  const src = [
    { name: 'a', extensions: {
      KHR_materials_clearcoat: { clearcoatFactor: 1 },
      KHR_materials_transmission: {},
      KHR_materials_volume: { thicknessFactor: 1 },
      KHR_materials_ior: {},
      KHR_materials_specular: {},
      KHR_materials_sheen: { sheenRoughnessFactor: 0.5 },
      KHR_materials_iridescence: { iridescenceFactor: 1 },
      KHR_materials_anisotropy: { anisotropyStrength: 0.5 },
      KHR_materials_dispersion: {},
    } },
  ];
  const out = [
    { name: 'a', extensions: {
      KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0 },
      KHR_materials_transmission: { transmissionFactor: 0 },
      KHR_materials_volume: { thicknessFactor: 1, attenuationColor: [1, 1, 1] },
      KHR_materials_ior: { ior: 1.5 },
      KHR_materials_specular: { specularFactor: 1, specularColorFactor: [1, 1, 1] },
      KHR_materials_sheen: { sheenRoughnessFactor: 0.5, sheenColorFactor: [0, 0, 0] },
      KHR_materials_iridescence: { iridescenceFactor: 1, iridescenceIor: 1.3, iridescenceThicknessMinimum: 100, iridescenceThicknessMaximum: 400 },
      KHR_materials_anisotropy: { anisotropyStrength: 0.5, anisotropyRotation: 0 },
      KHR_materials_dispersion: { dispersion: 0 },
    } },
  ];
  const r = compareFiles(gltf({ materials: src }), gltf({ materials: out }));
  assert.deepEqual(r.diffs, []);
  assert.equal(r.verdict, 'pass');
  const nine = ['anisotropy', 'clearcoat', 'dispersion', 'ior', 'iridescence', 'sheen', 'specular', 'transmission', 'volume'];
  assert.deepEqual(r.materials.map((m) => m.layers), [{ src: nine, out: nine }]);
});

test('a changed value is a difference, named by material, layer and field', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'm', extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } } }] }),
    gltf({ materials: [{ name: 'm', extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.25 } } }] }),
  );
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs, [{ material: 'm', layer: 'sheen', field: 'sheenRoughnessFactor', src: 0.5, out: 0.25, kind: 'value' }]);
});

test('a default against a changed value is a difference (the default is filled, not skipped)', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'g', extensions: { KHR_materials_ior: {} } }] }),
    gltf({ materials: [{ name: 'g', extensions: { KHR_materials_ior: { ior: 1.4 } } }] }),
  );
  assert.deepEqual(r.diffs, [{ material: 'g', layer: 'ior', field: 'ior', src: 1.5, out: 1.4, kind: 'value' }]);
});

// Ruling A20a: floats compare as f32. Two numbers that round to the same f32
// are equal (the scene holds f32); each such pair is listed as information.
test('f32 rounding of a JSON number passes, listed as information', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'c', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0.03 } } }] }),
    gltf({ materials: [{ name: 'c', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0.029999999329447746 } } }] }),
  );
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.diffs, []);
  assert.deepEqual(r.f32Rounded, [{ material: 'c', layer: 'clearcoat', field: 'clearcoatRoughnessFactor', src: 0.03, out: 0.029999999329447746 }]);
});

test('f32 rounding inside an array factor is listed per element', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 's', extensions: { KHR_materials_sheen: { sheenColorFactor: [1, 0.329, 0.1] } } }] }),
    gltf({ materials: [{ name: 's', extensions: { KHR_materials_sheen: { sheenColorFactor: [1, 0.32899999618530273, 0.10000000149011612] } } }] }),
  );
  assert.deepEqual(r.diffs, []);
  assert.deepEqual(r.f32Rounded, [
    { material: 's', layer: 'sheen', field: 'sheenColorFactor[1]', src: 0.329, out: 0.32899999618530273 },
    { material: 's', layer: 'sheen', field: 'sheenColorFactor[2]', src: 0.1, out: 0.10000000149011612 },
  ]);
});

test('a difference past f32 rounding still fails, however small', () => {
  // 0.03 and 0.0300001 round to different f32s (0.029999999329447746 and
  // 0.030000099912285805): a tolerance would hide this, f32 equality does not.
  const r = compareFiles(
    gltf({ materials: [{ name: 'c', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0.03 } } }] }),
    gltf({ materials: [{ name: 'c', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatRoughnessFactor: 0.0300001 } } }] }),
  );
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs, [{ material: 'c', layer: 'clearcoat', field: 'clearcoatRoughnessFactor', src: 0.03, out: 0.0300001, kind: 'value' }]);
  assert.deepEqual(r.f32Rounded, []);
});

test('a dropped layer and an added layer both fail', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'v', extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 }, KHR_materials_ior: { ior: 1.4 } } }] }),
    gltf({ materials: [{ name: 'v', extensions: { KHR_materials_ior: { ior: 1.4 }, KHR_materials_dispersion: { dispersion: 1 } } }] }),
  );
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs, [
    { material: 'v', layer: 'dispersion', field: '(layer)', src: null, out: 'present', kind: 'layer-added' },
    { material: 'v', layer: 'sheen', field: '(layer)', src: 'present', out: null, kind: 'layer-missing' },
  ]);
});

test('a field the schema does not name is compared too', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'x', extensions: { KHR_materials_ior: { ior: 1.4, vendorField: 2 } } }] }),
    gltf({ materials: [{ name: 'x', extensions: { KHR_materials_ior: { ior: 1.4 } } }] }),
  );
  assert.deepEqual(r.diffs, [{ material: 'x', layer: 'ior', field: 'vendorField', src: 2, out: null, kind: 'value' }]);
});

// ─── textures ───────────────────────────────────────────────────────────────

const A = [1, 2, 3, 4];
const B = [9, 9, 9];
const SHA_A = '9f64a747e1b97f131fabb6b447296c9b6f0201e79fb3c5356e6c77e89b6a806a'; // printf '\x01\x02\x03\x04' | shasum -a 256
const SHA_B = 'e740a6faf2db65f5853148d75d9a335d7c4b94ab106fe5f237bc34fdcfc74584'; // printf '\x09\x09\x09' | shasum -a 256

test('textures compare by the bytes their image holds, not by index', () => {
  // The source's coat map is texture 1 -> image 1 (bytes A); the output's is
  // texture 0 -> image 0 (bytes A).
  const r = compareFiles(
    gltf({ images: [B, A], textures: [{ source: 0 }, { source: 1 }], materials: [{ name: 't', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 1 } } } }] }),
    gltf({ images: [A, B], textures: [{ source: 0 }, { source: 1 }], materials: [{ name: 't', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 0 } } } }] }),
  );
  assert.deepEqual(r.diffs, []);
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.materials.map((m) => m.layers), [{ src: ['clearcoat'], out: ['clearcoat'] }]);
});

test('a texture resolving to other bytes, and a texture lost, both fail', () => {
  const r = compareFiles(
    gltf({ images: [A, B], textures: [{ source: 0 }, { source: 1 }], materials: [{ name: 't', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 0 }, clearcoatRoughnessTexture: { index: 1 } } } }] }),
    gltf({ images: [B], textures: [{ source: 0 }], materials: [{ name: 't', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 0 } } } }] }),
  );
  assert.deepEqual(r.diffs, [
    { material: 't', layer: 'clearcoat', field: 'clearcoatRoughnessTexture', src: `image ${SHA_B}`, out: null, kind: 'texture-missing' },
    { material: 't', layer: 'clearcoat', field: 'clearcoatTexture.image', src: SHA_A, out: SHA_B, kind: 'texture-image' },
  ]);
});

test('texCoord: absent equals 0, and 1 differs from 0', () => {
  const tex = (info) => [{ name: 'u', extensions: { KHR_materials_sheen: { sheenColorFactor: [1, 1, 1], sheenColorTexture: info } } }];
  const same = compareFiles(gltf({ images: [A], textures: [{ source: 0 }], materials: tex({ index: 0 }) }), gltf({ images: [A], textures: [{ source: 0 }], materials: tex({ index: 0, texCoord: 0 }) }));
  assert.deepEqual(same.diffs, []);
  const moved = compareFiles(gltf({ images: [A], textures: [{ source: 0 }], materials: tex({ index: 0 }) }), gltf({ images: [A], textures: [{ source: 0 }], materials: tex({ index: 0, texCoord: 1 }) }));
  assert.deepEqual(moved.diffs, [{ material: 'u', layer: 'sheen', field: 'sheenColorTexture.texCoord', src: 0, out: 1, kind: 'value' }]);
});

test('KHR_texture_transform: absent equals identity, a changed offset differs, and its texCoord overrides', () => {
  const tex = (info) => [{ name: 'k', extensions: { KHR_materials_sheen: { sheenColorFactor: [1, 1, 1], sheenColorTexture: info } } }];
  const file = (info) => gltf({ images: [A], textures: [{ source: 0 }], materials: tex(info) });
  const identity = { index: 0, extensions: { KHR_texture_transform: { offset: [0, 0], rotation: 0, scale: [1, 1] } } };
  assert.deepEqual(compareFiles(file({ index: 0 }), file(identity)).diffs, []);
  const shifted = { index: 0, extensions: { KHR_texture_transform: { offset: [0.5, 0], scale: [2, 2] } } };
  assert.deepEqual(compareFiles(file({ index: 0 }), file(shifted)).diffs, [
    { material: 'k', layer: 'sheen', field: 'sheenColorTexture.transform.offset[0]', src: 0, out: 0.5, kind: 'value' },
    { material: 'k', layer: 'sheen', field: 'sheenColorTexture.transform.scale[0]', src: 1, out: 2, kind: 'value' },
    { material: 'k', layer: 'sheen', field: 'sheenColorTexture.transform.scale[1]', src: 1, out: 2, kind: 'value' },
  ]);
  // The transform's own texCoord wins over the textureInfo's (KHR_texture_transform: "Overrides the textureInfo texCoord value if supplied").
  const overridden = { index: 0, texCoord: 0, extensions: { KHR_texture_transform: { texCoord: 1 } } };
  assert.deepEqual(compareFiles(file(overridden), file({ index: 0, texCoord: 1 })).diffs, []);
});

test('a texture whose image resolves on neither side fails, never equal', () => {
  // Texture 0 names image 5 in both files, and neither file has one: the
  // two "unresolved" texts are alike, and must not pass as the same image.
  const coat = () => gltf({ images: [A], textures: [{ source: 5 }], materials: [{ name: 'u', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 0 } } } }] });
  const r = compareFiles(coat(), coat());
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs, [
    { material: 'u', layer: 'clearcoat', field: 'clearcoatTexture.image', src: 'unresolved (texture 0, image 5)', out: 'unresolved (texture 0, image 5)', kind: 'texture-unresolved' },
  ]);
});

test("a normal texture's scale defaults to 1 and is compared", () => {
  const coat = (info) => gltf({ images: [A], textures: [{ source: 0 }], materials: [{ name: 'n', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatNormalTexture: info } } }] });
  assert.deepEqual(compareFiles(coat({ index: 0 }), coat({ index: 0, scale: 1 })).diffs, []);
  assert.deepEqual(compareFiles(coat({ index: 0 }), coat({ index: 0, scale: 0.5 })).diffs, [
    { material: 'n', layer: 'clearcoat', field: 'clearcoatNormalTexture.scale', src: 1, out: 0.5, kind: 'value' },
  ]);
});

test('textures inside a GLB resolve through its bufferViews', () => {
  const m = (index) => [{ name: 'g', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index } } } }];
  const same = compareFiles(glb({ images: [B, A], materials: m(1) }), glb({ images: [A], materials: m(0) }));
  assert.deepEqual(same.diffs, []);
  const other = compareFiles(glb({ images: [B, A], materials: m(1) }), glb({ images: [B], materials: m(0) }));
  assert.deepEqual(other.diffs, [{ material: 'g', layer: 'clearcoat', field: 'clearcoatTexture.image', src: SHA_A, out: SHA_B, kind: 'texture-image' }]);
});

// ─── what is excluded, and what is listed ───────────────────────────────────

test("extras and a layer object's own nested extensions are excluded and listed", () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'e', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, extras: { note: 1 }, extensions: { EXT_vendor_coat: {} } } } }] }),
    gltf({ materials: [{ name: 'e', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1 } } }] }),
  );
  assert.deepEqual(r.diffs, []);
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.excluded, [
    { material: 'e', path: 'KHR_materials_clearcoat.extras' },
    { material: 'e', path: 'KHR_materials_clearcoat.extensions.EXT_vendor_coat' },
  ]);
});

test('other material extensions are listed with whether litegen kept them', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'a', extensions: { KHR_materials_emissive_strength: { emissiveStrength: 4 } } }, { name: 'b', extensions: { KHR_materials_unlit: {} } }] }),
    gltf({ materials: [{ name: 'a', extensions: { KHR_materials_emissive_strength: { emissiveStrength: 4 } } }, { name: 'b' }] }),
  );
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.otherExtensions, { KHR_materials_emissive_strength: { src: 1, kept: 1 }, KHR_materials_unlit: { src: 1, kept: 0 } });
});

// ─── pairing materials ──────────────────────────────────────────────────────

test('unnamed materials pair by index when the counts match', () => {
  const m = (ior) => [{ extensions: { KHR_materials_ior: { ior: 1.4 } } }, { extensions: { KHR_materials_ior: { ior } } }];
  const r = compareFiles(gltf({ materials: m(1.2) }), gltf({ materials: m(1.3) }));
  assert.equal(r.pairing, 'index');
  assert.deepEqual(r.diffs, [{ material: '#1', layer: 'ior', field: 'ior', src: 1.2, out: 1.3, kind: 'value' }]);
});

test('materials pair by name when the counts differ; a dropped variant-only material is listed, a dropped bound one fails', () => {
  const coated = { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } };
  const src = {
    materials: [{ name: 'A', extensions: coated }, { name: 'B', extensions: coated }, { name: 'C' }, { name: 'D' }],
    bound: [0, 2, 3],
    variants: [{ material: 1, variants: [1] }],
  };
  const kept = compareFiles(gltf(src), gltf({ materials: [{ name: 'A', extensions: coated }, { name: 'C' }, { name: 'D' }], bound: [0, 1, 2] }));
  assert.equal(kept.pairing, 'name');
  assert.deepEqual(kept.diffs, []);
  assert.equal(kept.verdict, 'pass');
  assert.deepEqual(kept.variantOnly, ['B']);
  assert.deepEqual(kept.unpaired, { src: [], out: [] });
  const lost = compareFiles(gltf(src), gltf({ materials: [{ name: 'A', extensions: coated }, { name: 'C' }], bound: [0, 1] }));
  assert.equal(lost.verdict, 'fail');
  assert.deepEqual(lost.unpaired, { src: ['D'], out: [] });
});

test('an unpaired output material is judged by its own layers, not by a namesake label', () => {
  // The source's one material, named "#1", pairs by name with the output's
  // first. The output's second, unnamed, is labelled "#1" too, and carries a
  // sheen the source never had: it fails, whatever the first one carries.
  const r = compareFiles(
    gltf({ materials: [{ name: '#1' }] }),
    gltf({ materials: [{ name: '#1' }, { extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } } }] }),
  );
  assert.equal(r.pairing, 'name');
  assert.deepEqual(r.unpaired, { src: [], out: ['#1'] });
  assert.equal(r.verdict, 'fail');
});

test('KHR_materials_variants is detected at the root and on primitives', () => {
  const r = compareFiles(
    gltf({ materials: [{ name: 'A' }, { name: 'B' }], bound: [0], variants: [{ material: 1, variants: [1] }] }),
    gltf({ materials: [{ name: 'A' }], bound: [0] }),
  );
  assert.deepEqual(r.variants, { root: true, primitives: 1, kept: false });
});

// Followups 2026-11-08, F: litegen's reader allocates a material when the
// first primitive that uses it is read (glb.rs `material`, L2191-L2266, called
// at the end of `primitive`, L2171-L2175), walking the scene's nodes depth-first
// (`nodes`, L2794-L2864; pass 2 reads each node's mesh at its first sight,
// L2868-L2873), and the writer writes `scene.materials` in that order
// (L4040-L4121). So a file whose materials are not in order of first use comes
// back reordered, and its repeated names cannot pair by name or by index.
// Khronos's VirtualCity has 167 materials with 33 names, 16 used more than
// once. Here: three materials, two named 'Rep', the first used last.
const VC_SRC = [
  { name: 'Rep', extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } } },
  { name: 'Rep' },
  { name: 'Other' },
];
// The scene: nodes 1 then 0 under the root (so mesh 1 is read first), mesh 0
// uses material 0, mesh 1 materials 2 then 1. Reader order: 2, 1, 0.
const vcRaw = {
  scenes: [{ nodes: [2] }],
  nodes: [{ mesh: 0 }, { mesh: 1 }, { children: [1, 0] }],
  meshes: [{ primitives: [{ attributes: { POSITION: 0 }, material: 0 }] }, { primitives: [{ attributes: { POSITION: 0 }, material: 2 }, { attributes: { POSITION: 0 }, material: 1 }] }],
};
const vcOut = (sheenAt) => [
  { name: 'Other' },
  { name: 'Rep', ...(sheenAt === 1 ? { extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } } } : {}) },
  { name: 'Rep', ...(sheenAt === 2 ? { extensions: { KHR_materials_sheen: { sheenRoughnessFactor: 0.5 } } } : {}) },
];

test("repeated material names pair one to one, in the order litegen's reader keeps the materials", () => {
  const r = compareFiles(gltf({ materials: VC_SRC, raw: vcRaw }), gltf({ materials: vcOut(2) }));
  assert.equal(r.pairing, 'reader order');
  assert.equal(r.verdict, 'pass', JSON.stringify(r.diffs) + JSON.stringify(r.unpaired));
  assert.deepEqual(r.unpaired, { src: [], out: [] });
  // Source 0 is the output's third, 1 its second, 2 its first.
  assert.deepEqual(r.materials.map((m) => [m.src, m.out]), [[0, 2], [1, 1], [2, 0]]);
  assert.deepEqual(r.materials[0].layers, { src: ['sheen'], out: ['sheen'] });
});

test('a layer on the wrong one of two same-named materials still fails', () => {
  // Negative control: the output's sheen on the second Rep, not the third.
  const r = compareFiles(gltf({ materials: VC_SRC, raw: vcRaw }), gltf({ materials: vcOut(1) }));
  assert.equal(r.pairing, 'reader order');
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs.map((d) => [d.material, d.kind]), [['Rep', 'layer-missing'], ['Rep', 'layer-added']]);
});

test('without that order the repeated names stay unpaired, as before', () => {
  // The output in the file's own order, not the reader's: names disagree
  // with the reader order at index 0, so the pairing falls back to names, and
  // a repeated name pairs with nothing.
  const r = compareFiles(gltf({ materials: VC_SRC, raw: vcRaw }), gltf({ materials: [{ name: 'Rep' }, { name: 'Rep' }, { name: 'Other' }] }));
  assert.equal(r.pairing, 'index', 'the file order is the output order here: names agree at every index');
  const swapped = compareFiles(gltf({ materials: VC_SRC, raw: vcRaw }), gltf({ materials: [{ name: 'Rep' }, { name: 'Other' }, { name: 'Other' }] }));
  assert.equal(swapped.pairing, 'name');
  assert.equal(swapped.verdict, 'fail');
  assert.ok(swapped.unpaired.src.includes('Rep'));
});

test("a file with no scene is read mesh by mesh, as the reader reads it; the scene's depth-first order decides otherwise", () => {
  // No scene: every mesh in file order (glb.rs L2800-L2807), so the order of
  // first use is the order of the primitives: 2, 1, 0.
  const noScene = { meshes: vcRaw.meshes.slice().reverse() };
  const r = compareFiles(gltf({ materials: VC_SRC, raw: noScene }), gltf({ materials: vcOut(2) }));
  assert.equal(r.pairing, 'reader order');
  assert.equal(r.verdict, 'pass');
  // A scene whose roots are [0, 1] reads mesh 0 first: 0, 2, 1.
  const forward = { ...vcRaw, scenes: [{ nodes: [0, 1] }] };
  const out = [VC_SRC[0], { name: 'Other' }, { name: 'Rep' }];
  const f = compareFiles(gltf({ materials: VC_SRC, raw: forward }), gltf({ materials: out }));
  assert.equal(f.pairing, 'reader order');
  assert.equal(f.verdict, 'pass');
  assert.deepEqual(f.materials.map((m) => [m.src, m.out]), [[0, 0], [1, 2], [2, 1]]);
});

// ─── the defaults table against the Khronos schemas ─────────────────────────

// The schema files are Khronos's (KhronosGroup/glTF at 9d23a5a5), not the
// repo's: KHRONOS_SCHEMAS names a folder of them, and without it this one
// test is skipped, saying so (README, "The layer-extension check").
const SCHEMAS = process.env.KHRONOS_SCHEMAS;
const NO_SCHEMAS =
  'KHRONOS_SCHEMAS names no folder of the nine Khronos schema files; scripts/mesh-verify/README.md, "The layer-extension check", says how to fetch and check them';

test('the defaults table equals the Khronos schema files, property for property', { skip: SCHEMAS ? false : NO_SCHEMAS }, () => {
  const dir = SCHEMAS;
  for (const layer of ['clearcoat', 'transmission', 'volume', 'ior', 'specular', 'sheen', 'iridescence', 'anisotropy', 'dispersion']) {
    const ext = `KHR_materials_${layer}`;
    const schema = JSON.parse(fs.readFileSync(path.join(dir, `material.${ext}.schema.json`), 'utf8'));
    const want = {};
    for (const [k, v] of Object.entries(schema.properties)) {
      if (k === 'extensions' || k === 'extras' || v.allOf) continue; // textures carry no default
      want[k] = v.default;
    }
    if (layer === 'volume') {
      // No `default` key: "When undefined, the value is assumed to be infinite."
      assert.match(schema.properties.attenuationDistance.gltf_detailedDescription, /When undefined, the value is assumed to be infinite/);
      want.attenuationDistance = Infinity;
    }
    assert.deepEqual(DEFAULTS[ext], want, ext);
  }
});

// ─── helpers (test-only) ────────────────────────────────────────────────────

// `bound`: the material indices a primitive references (all of them by
// default). `variants`: KHR_materials_variants mappings on one extra
// primitive, as SheenChair carries them. `raw`: members laid over the
// document last (a scene graph and its own meshes).
function doc({ materials, textures, images, bound, variants, raw }, imageEntry) {
  const j = { asset: { version: '2.0' }, materials };
  if (images) {
    j.images = images.map(imageEntry);
    j.textures = textures || images.map((_, i) => ({ source: i }));
  }
  const prims = (bound ?? materials.map((_, i) => i)).map((material) => ({ attributes: { POSITION: 0 }, material }));
  if (variants) {
    prims.push({ attributes: { POSITION: 0 }, material: bound?.[0] ?? 0, extensions: { KHR_materials_variants: { mappings: variants } } });
    j.extensions = { KHR_materials_variants: { variants: [{ name: 'one' }, { name: 'two' }] } };
  }
  j.meshes = [{ primitives: prims }];
  return Object.assign(j, raw);
}

function gltf(spec) {
  const j = doc(spec, (bytes) => ({ uri: `data:application/octet-stream;base64,${Buffer.from(bytes).toString('base64')}` }));
  const file = path.join(tmp, `f${n++}.gltf`);
  fs.writeFileSync(file, JSON.stringify(j));
  return file;
}

function glb(spec) {
  let bin = Buffer.alloc(0);
  const views = [];
  const j = doc(spec, (bytes) => {
    const at = bin.length;
    bin = Buffer.concat([bin, Buffer.from(bytes), Buffer.alloc((4 - (bytes.length % 4)) % 4)]);
    views.push({ buffer: 0, byteOffset: at, byteLength: bytes.length });
    return { bufferView: views.length - 1, mimeType: 'image/png' };
  });
  j.bufferViews = views;
  j.buffers = [{ byteLength: bin.length }];
  let text = Buffer.from(JSON.stringify(j));
  text = Buffer.concat([text, Buffer.alloc((4 - (text.length % 4)) % 4, 0x20)]);
  const head = Buffer.alloc(12);
  head.write('glTF', 0);
  head.writeUInt32LE(2, 4);
  head.writeUInt32LE(12 + 8 + text.length + 8 + bin.length, 8);
  const chunk = (type, body) => {
    const h = Buffer.alloc(8);
    h.writeUInt32LE(body.length, 0);
    h.write(type, 4);
    return Buffer.concat([h, body]);
  };
  const file = path.join(tmp, `f${n++}.glb`);
  fs.writeFileSync(file, Buffer.concat([head, chunk('JSON', text), chunk('BIN\0', bin)]));
  return file;
}
