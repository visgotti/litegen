// Tests for pixel-digest.mjs and for the one difference layer-extensions.mjs
// excuses with it: a KTX2 image of a source glTF that litegen wrote as PNG
// (mesh-gaps item 3). Run, from the repo, after `npm ci` in scripts/mesh-verify
// (three's Binomial transcoder is read from its node_modules):
//
//   node --test scripts/mesh-verify/pixel-digest.test.mjs
//
// Every expectation is a literal or computed by the test's own few lines, not
// by the code under test. The one KTX2 file is Khronos's CarConcept `Rib_N`
// (CC BY 4.0, litegen-core/src/mesh/testdata/ktx2/), 32 x 32 UASTC.
import { after, before, test } from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { compareFiles, usePixelDigest } from './layer-extensions.mjs';
import { loadPixelDigest, loadPixelReader } from './pixel-digest.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const RIB_N = fs.readFileSync(path.join(here, '..', '..', 'litegen-core', 'src', 'mesh', 'testdata', 'ktx2', 'Rib_N.ktx2'));
const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'pixel-digest-test-'));
after(() => {
  usePixelDigest(null);
  fs.rmSync(tmp, { recursive: true, force: true });
});
let read;
let digest;
before(async () => {
  read = await loadPixelReader();
  digest = await loadPixelDigest();
});

// ─── a PNG, built here ──────────────────────────────────────────────────────

const CHANNELS = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 };
const chunk = (type, body) => {
  const head = Buffer.alloc(8);
  head.writeUInt32BE(body.length, 0);
  head.write(type, 4, 'latin1');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(Buffer.concat([head.subarray(4), body])), 0);
  return Buffer.concat([head, body, crc]);
};

// An 8-bit PNG of `px` (width x height x channels bytes), each row filtered by
// `filter(y)` (0 None, 1 Sub, 2 Up, 3 Average, 4 Paeth), written from the
// specification's definitions, not from the reader's code.
function png(width, height, color, px, { filter = () => 0, palette, trns } = {}) {
  const ch = CHANNELS[color];
  const stride = width * ch;
  const raw = Buffer.alloc((stride + 1) * height);
  const at = (x, y) => (x < 0 || y < 0 ? 0 : px[y * stride + x]);
  for (let y = 0; y < height; y++) {
    const f = filter(y);
    raw[y * (stride + 1)] = f;
    for (let x = 0; x < stride; x++) {
      const [a, b, c] = [at(x - ch, y), at(x, y - 1), at(x - ch, y - 1)];
      let predicted = 0;
      if (f === 1) predicted = a;
      else if (f === 2) predicted = b;
      else if (f === 3) predicted = Math.floor((a + b) / 2);
      else if (f === 4) {
        const p = a + b - c;
        const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
        predicted = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      raw[y * (stride + 1) + 1 + x] = (px[y * stride + x] - predicted) & 255;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = color;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    ...(palette ? [chunk('PLTE', Buffer.from(palette))] : []),
    ...(trns ? [chunk('tRNS', Buffer.from(trns))] : []),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

// A deterministic byte stream (xorshift32).
function bytes(n, seed) {
  const out = Buffer.alloc(n);
  let s = seed;
  for (let i = 0; i < n; i++) {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    out[i] = s & 255;
  }
  return out;
}

test('a PNG of every colour type, under every filter, reads back as its RGBA8', () => {
  const [w, h] = [19, 11];
  const cases = {
    6: (px) => px,
    2: (px) => Buffer.concat(Array.from({ length: w * h }, (_, i) => Buffer.from([px[i * 3], px[i * 3 + 1], px[i * 3 + 2], 255]))),
    0: (px) => Buffer.concat(Array.from({ length: w * h }, (_, i) => Buffer.from([px[i], px[i], px[i], 255]))),
    4: (px) => Buffer.concat(Array.from({ length: w * h }, (_, i) => Buffer.from([px[i * 2], px[i * 2], px[i * 2], px[i * 2 + 1]]))),
  };
  for (const [color, expand] of Object.entries(cases)) {
    const px = bytes(w * h * CHANNELS[color], 0x9e3779b9 + Number(color));
    const img = read(png(w, h, Number(color), px, { filter: (y) => y % 5 }));
    assert.deepEqual([img.width, img.height], [w, h], `colour type ${color}`);
    assert.deepEqual(Buffer.from(img.rgba), expand(px), `colour type ${color}`);
  }
  // Palette: two entries, the second one translucent.
  const index = Buffer.from(Array.from({ length: 6 }, (_, i) => i % 2));
  const img = read(png(3, 2, 3, index, { filter: (y) => 4 - y, palette: [10, 20, 30, 40, 50, 60], trns: [255, 7] }));
  assert.deepEqual(Buffer.from(img.rgba), Buffer.concat(Array.from({ length: 6 }, (_, i) => Buffer.from(i % 2 ? [40, 50, 60, 7] : [10, 20, 30, 255]))));
});

test('what is not a readable image reads as nothing', () => {
  assert.equal(read(Buffer.from('not an image')), null);
  assert.equal(digest(Buffer.alloc(0)), null);
  const good = png(2, 2, 6, bytes(16, 7));
  assert.equal(read(good.subarray(0, good.length - 20)), null, 'a truncated PNG');
  const sixteen = Buffer.from(good);
  sixteen[24] = 16; // IHDR bit depth
  assert.equal(read(sixteen), null, 'a 16-bit PNG is not read');
});

// ─── KTX2 against PNG ───────────────────────────────────────────────────────

test('a KTX2 image and a PNG of its pixels have one digest, and one changed byte makes another', () => {
  const ktx = read(RIB_N);
  assert.deepEqual([ktx.width, ktx.height, ktx.rgba.length], [32, 32, 32 * 32 * 4]);
  const same = png(32, 32, 6, Buffer.from(ktx.rgba), { filter: (y) => y % 5 });
  assert.equal(digest(same), digest(RIB_N));
  assert.match(digest(RIB_N), /^rgba8 32x32 [0-9a-f]{64}$/);
  // The digest is the sha-256 of the RGBA8 pixels, as the test says it is.
  assert.equal(digest(RIB_N), `rgba8 32x32 ${crypto.createHash('sha256').update(ktx.rgba).digest('hex')}`);
  const other = Buffer.from(ktx.rgba);
  other[1000] ^= 1;
  assert.notEqual(digest(png(32, 32, 6, other)), digest(RIB_N));
});

// ─── what layer-extensions.mjs does with it ─────────────────────────────────

let n = 0;
// A glTF with one material whose clearcoat texture draws `image`; `basisu`
// puts the image in KHR_texture_basisu as a KTX2 source does.
function gltf(image, basisu) {
  const j = {
    asset: { version: '2.0' },
    materials: [{ name: 'coated', extensions: { KHR_materials_clearcoat: { clearcoatFactor: 1, clearcoatTexture: { index: 0 } } } }],
    images: [{ uri: `data:application/octet-stream;base64,${image.toString('base64')}` }],
    textures: [basisu ? { extensions: { KHR_texture_basisu: { source: 0 } } } : { source: 0 }],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 }, material: 0 }] }],
  };
  const file = path.join(tmp, `f${n++}.gltf`);
  fs.writeFileSync(file, JSON.stringify(j));
  return file;
}

test('a KTX2 source image and a PNG of the same pixels pass, listed as information', () => {
  usePixelDigest(digest);
  const pixels = Buffer.from(read(RIB_N).rgba);
  const r = compareFiles(gltf(RIB_N, true), gltf(png(32, 32, 6, pixels), false));
  assert.equal(r.verdict, 'pass');
  assert.deepEqual(r.diffs, []);
  assert.equal(r.ktx2Pixels.length, 1);
  assert.deepEqual([r.ktx2Pixels[0].material, r.ktx2Pixels[0].layer, r.ktx2Pixels[0].field], ['coated', 'clearcoat', 'clearcoatTexture.image']);
  assert.equal(r.ktx2Pixels[0].pixels, digest(RIB_N));
});

test('a KTX2 source image against a PNG with one pixel changed fails', () => {
  usePixelDigest(digest);
  const pixels = Buffer.from(read(RIB_N).rgba);
  pixels[2000] ^= 1;
  const r = compareFiles(gltf(RIB_N, true), gltf(png(32, 32, 6, pixels), false));
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs.map((d) => [d.field, d.kind]), [['clearcoatTexture.image', 'texture-image']]);
  assert.deepEqual(r.ktx2Pixels, []);
});

test('without the digest a KTX2 source image against its PNG is the byte difference it looks like', () => {
  usePixelDigest(null);
  const r = compareFiles(gltf(RIB_N, true), gltf(png(32, 32, 6, Buffer.from(read(RIB_N).rgba)), false));
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs.map((d) => d.kind), ['texture-image']);
});

test('only a KTX2 source is excused: two PNGs of the same pixels with other bytes still differ, and equal KTX2 bytes are equal', () => {
  usePixelDigest(digest);
  const pixels = Buffer.from(read(RIB_N).rgba);
  const plain = compareFiles(gltf(png(32, 32, 6, pixels), false), gltf(png(32, 32, 6, pixels, { filter: () => 1 }), false));
  assert.equal(plain.verdict, 'fail');
  assert.deepEqual(plain.diffs.map((d) => d.kind), ['texture-image']);
  const kept = compareFiles(gltf(RIB_N, true), gltf(RIB_N, true));
  assert.equal(kept.verdict, 'pass');
  assert.deepEqual(kept.ktx2Pixels, []);
});

test('a PNG source with a KTX2 output of the same pixels still fails: the difference is by design only the other way', () => {
  usePixelDigest(digest);
  const pixels = Buffer.from(read(RIB_N).rgba);
  const r = compareFiles(gltf(png(32, 32, 6, pixels), false), gltf(RIB_N, true));
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs.map((d) => [d.field, d.kind]), [['clearcoatTexture.image', 'texture-image']]);
  assert.deepEqual(r.ktx2Pixels, []);
});

test('a KTX2 source against an output PNG the reader cannot read fails as the byte difference', () => {
  usePixelDigest(digest);
  const sixteen = png(32, 32, 6, Buffer.from(read(RIB_N).rgba));
  sixteen[24] = 16; // IHDR bit depth: not read, so no digest
  const r = compareFiles(gltf(RIB_N, true), gltf(sixteen, false));
  assert.equal(r.verdict, 'fail');
  assert.deepEqual(r.diffs.map((d) => [d.field, d.kind]), [['clearcoatTexture.image', 'texture-image']]);
  assert.deepEqual(r.ktx2Pixels, []);
});

test('an IDAT that inflates past what its header declares reads as nothing', () => {
  const pixels = bytes(16, 3);
  const raw = Buffer.alloc((2 * 4 + 1) * 2 + 4096); // 2 x 2 RGBA declares 18 bytes; this inflates to 4,114
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(2, 0);
  ihdr.writeUInt32BE(2, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const over = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw)),
    chunk('IEND', Buffer.alloc(0)),
  ]);
  assert.equal(read(over), null);
  assert.equal(digest(over), null);
  assert.notEqual(read(png(2, 2, 6, pixels)), null, 'the same header with the right amount reads');
});
