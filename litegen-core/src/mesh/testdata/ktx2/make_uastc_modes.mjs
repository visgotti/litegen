#!/usr/bin/env node
// make_uastc_modes.mjs: writes litegen's own UASTC test vector, `uastc_all_modes.ktx2`, and the
// pixels the Basis Universal transcoder gives for it, `uastc_all_modes.rgba`.
//
// The Khronos samples hold few UASTC blocks of few modes (a normal map is mostly one mode), so
// they cannot show that every one of UASTC's 19 modes decodes bit for bit. This file does: it is
// a 62 x 59 UASTC LDR 4x4 texture (16 x 15 = 240 blocks, so the right column of blocks is cut to
// 2 pixels and the bottom row to 3), not supercompressed (scheme 0), whose blocks are
// pseudo-random bits with the fields that pick a path set on purpose:
//
//   - every mode but the reserved code 19 appears (modes 0 to 18, mode 8 being the solid colour);
//   - every common partition pattern: modes 2, 4, 9 and 16 with all 30 of the two-subset
//     patterns, mode 3 with all 11 three-subset ones, mode 7 with all 19 of its own;
//   - every colour component selector (0 to 3) in the dual-plane modes 6, 11 and 13, and mode 17;
//   - random ETC1/BC1 hint bits in every mode (a decoder must ignore them);
//   - random weights and endpoints, so every endpoint range and weight range is used with
//     extreme and middle values alike.
//
// The bits are our own: a xorshift64 stream seeded with 0x9E3779B97F4A7C15, no input from any
// image. The file is our content, with no third-party licence. Nothing here is an encoder's
// output; the transcoder is the judge of what each block means, so the reference is whatever
// Basis Universal v1.50 (three.js r183's bundled transcoder; see README.md) makes of the bits.
//
//   node make_uastc_modes.mjs <transcoder dir> <out dir>
//
//   <transcoder dir>  holds basis_transcoder.js and basis_transcoder.wasm, e.g.
//                     scripts/mesh-verify/node_modules/three/examples/jsm/libs/basis
//   <out dir>         an existing folder to write uastc_all_modes.ktx2 and uastc_all_modes.rgba into
//
// It is run by hand, once, and is not run in CI. It is deterministic: running it again writes the
// same two files. It checks that the transcoder accepts the file, that two transcodes agree, and
// that the plan covered everything listed above, and exits non-zero otherwise.
//
// It writes the raw uastc_all_modes.rgba (14,632 bytes); the repository holds it compressed as
// uastc_all_modes.rgba.zst, like every other reference (see make_reference.mjs and the README):
//
//   zstd -q -f --ultra -22 --long=27 -o uastc_all_modes.rgba.zst <out dir>/uastc_all_modes.rgba

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const RGBA32 = 13; // KTX2Loader.TranscoderFormat.RGBA32
const WIDTH = 62;
const HEIGHT = 59;
const BLOCKS_X = Math.ceil(WIDTH / 4);
const BLOCKS_Y = Math.ceil(HEIGHT / 4);
const NAME = 'uastc_all_modes';

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

// The mode prefix codes of UASTC (basisu_transcoder.cpp, g_uastc_mode_huff_codes): [code, bits],
// read from bit 0 of the block upward. Mode 19 is reserved ("future expansion") and is not used.
const MODE_CODES = [
  [0x1, 4], [0x35, 6], [0x1d, 5], [0x3, 5], [0x13, 5], [0xb, 5], [0x1b, 5], [0x7, 5], [0x17, 5], [0xf, 5],
  [0x2, 3], [0x0, 2], [0x6, 3], [0x1f, 5], [0xd, 5], [0x5, 7], [0x15, 6], [0x25, 6], [0x9, 4],
];
// The ETC1/BC1/ETC2 hint bits that follow the mode code in each mode (g_uastc_mode_total_hint_bits).
// A solid-colour block has none that matter.
const HINT_BITS = [15, 15, 15, 15, 15, 15, 15, 15, 0, 23, 17, 17, 17, 23, 23, 23, 23, 23, 15];
// How many common partition patterns each mode that has one chooses from, and how many bits name one.
const PATTERNS = { 2: [30, 5], 3: [11, 4], 4: [30, 5], 7: [19, 5], 9: [30, 5], 16: [30, 5] };
// The modes with a colour component selector (2 bits after the hints), and mode 17, whose is fixed.
const CCS_MODES = [6, 11, 13];

// xorshift64 (Marsaglia, 2003), the generator the sweeps in ktx2.rs use.
let state = 0x9e3779b97f4a7c15n;
const MASK64 = (1n << 64n) - 1n;
function next() {
  state ^= (state << 13n) & MASK64;
  state ^= state >> 7n;
  state ^= (state << 17n) & MASK64;
  return state;
}
const below = (n) => Number(next() % BigInt(n));

// 128 random bits as a BigInt, then fields written over them.
function randomBlock() {
  let v = 0n;
  for (let i = 0; i < 4; i++) v |= (next() & 0xffffffffn) << BigInt(32 * i);
  return v;
}
function setBits(v, at, count, value) {
  const mask = ((1n << BigInt(count)) - 1n) << BigInt(at);
  return (v & ~mask) | ((BigInt(value) << BigInt(at)) & mask);
}

function block(mode, pattern, ccs) {
  const [code, codeBits] = MODE_CODES[mode];
  let v = setBits(randomBlock(), 0, codeBits, code);
  const at = codeBits + HINT_BITS[mode]; // where the pattern or the selector starts
  if (mode in PATTERNS) {
    const [count, bits] = PATTERNS[mode];
    v = setBits(v, at, bits, pattern ?? below(count));
  }
  if (CCS_MODES.includes(mode)) v = setBits(v, at, 2, ccs ?? below(4));
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = Number((v >> BigInt(8 * i)) & 0xffn);
  return out;
}

// The plan: every pattern, every selector, every other mode twice, then random fill.
function plan() {
  const list = [];
  for (const [mode, [count]] of Object.entries(PATTERNS)) {
    for (let p = 0; p < count; p++) list.push({ mode: Number(mode), pattern: p });
  }
  for (const mode of CCS_MODES) for (let c = 0; c < 4; c++) list.push({ mode, ccs: c });
  for (let i = 0; i < 2; i++) list.push({ mode: 17 });
  for (const mode of [0, 1, 5, 8, 10, 12, 14, 15, 18]) for (let i = 0; i < 2; i++) list.push({ mode });
  while (list.length < BLOCKS_X * BLOCKS_Y) list.push({ mode: below(19) });
  if (list.length !== BLOCKS_X * BLOCKS_Y) throw new Error(`the plan has ${list.length} blocks, not ${BLOCKS_X * BLOCKS_Y}`);
  // Fisher-Yates, so no mode owns a whole row and the cut edge sees a mix.
  for (let i = list.length - 1; i > 0; i--) {
    const j = below(i + 1);
    [list[i], list[j]] = [list[j], list[i]];
  }
  return list;
}

// The KTX2 file: Identifier, Header, Index, one Level Index entry, a 44-byte DFD (UASTC, RGBA
// channel), then the blocks at the next multiple of 16 (the texel block size).
function ktx2(blocks) {
  const dfdAt = 80 + 24;
  const dfdLen = 44;
  const levelAt = Math.ceil((dfdAt + dfdLen) / 16) * 16;
  const levelLen = blocks.length * 16;
  const out = new Uint8Array(levelAt + levelLen);
  const dv = new DataView(out.buffer);
  out.set([0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a]);
  const header = [0, 1, WIDTH, HEIGHT, 0, 0, 1, 1, 0]; // vkFormat 0, typeSize 1, size, depth, layers, faces, levels, scheme 0
  header.forEach((v, i) => dv.setUint32(12 + 4 * i, v, true));
  dv.setUint32(48, dfdAt, true);
  dv.setUint32(52, dfdLen, true); // kvd and sgd: 0
  dv.setBigUint64(80, BigInt(levelAt), true);
  dv.setBigUint64(88, BigInt(levelLen), true);
  dv.setBigUint64(96, BigInt(levelLen), true);
  const dfd = [
    dfdLen, // dfdTotalSize
    0, // vendorId 0 (Khronos), descriptorType 0 (basic)
    2 | (40 << 16), // versionNumber 2, descriptorBlockSize 40
    166 | (1 << 8) | (1 << 16), // colorModel UASTC, primaries BT.709, transfer linear, flags 0
    3 | (3 << 8), // texelBlockDimension0..3: 4 x 4 (each one less than the size), 1 x 1 x 1 would be 0
    16, // bytesPlane0 = 16
    0, // bytesPlane4..7
    0 | (127 << 16) | (3 << 24), // one sample: bitOffset 0, bitLength 128, channel type 3 (RGBA)
    0, // samplePosition0..3
    0, // sampleLower
    0xffffffff, // sampleUpper
  ];
  dfd.forEach((v, i) => dv.setUint32(dfdAt + 4 * i, v >>> 0, true));
  blocks.forEach((b, i) => out.set(b, levelAt + 16 * i));
  return out;
}

// basis_transcoder.js is CommonJS inside a package of "type": "module", so this evaluates it the
// way three's KTX2Loader does (see make_reference.mjs).
async function loadBasis(dir) {
  const jsPath = path.join(dir, 'basis_transcoder.js');
  const source = fs.readFileSync(jsPath, 'utf8');
  const wasmBinary = new Uint8Array(fs.readFileSync(path.join(dir, 'basis_transcoder.wasm')));
  const mod = { exports: {} };
  const only = (name) => {
    if (name === 'fs') return fs;
    if (name === 'path') return path;
    throw new Error(`the transcoder asked for module ${name}`);
  };
  new Function('module', 'exports', 'require', '__filename', '__dirname', source)(mod, mod.exports, only, jsPath, dir);
  const BASIS = mod.exports;
  const basis = await new Promise((resolve, reject) => {
    const m = { wasmBinary, onRuntimeInitialized: () => resolve(m) };
    try {
      BASIS(m);
    } catch (e) {
      reject(e);
    }
  });
  basis.initializeBasis();
  if (basis.KTX2File === undefined) throw new Error('this transcoder has no KTX2File');
  return basis;
}

async function main() {
  const [dir, outDir] = process.argv.slice(2);
  if (!dir || !outDir) {
    console.error('usage: node make_uastc_modes.mjs <transcoder dir> <out dir>');
    process.exit(2);
  }
  if (!fs.statSync(outDir).isDirectory()) throw new Error(`${outDir} is not a folder`);

  const steps = plan();
  const blocks = steps.map((s) => block(s.mode, s.pattern, s.ccs));
  const file = ktx2(blocks);

  const basis = await loadBasis(dir);
  const ktx = new basis.KTX2File(file);
  let rgba;
  try {
    if (!ktx.isValid()) throw new Error('the transcoder says the file is invalid');
    if (!ktx.isUASTC() || ktx.getWidth() !== WIDTH || ktx.getHeight() !== HEIGHT || ktx.getLevels() !== 1) {
      throw new Error('the transcoder reads something other than the file written');
    }
    if (!ktx.startTranscoding()) throw new Error('startTranscoding failed');
    const size = ktx.getImageTranscodedSizeInBytes(0, 0, 0, RGBA32);
    if (size !== WIDTH * HEIGHT * 4) throw new Error(`RGBA32 size ${size} is not width x height x 4`);
    const transcode = () => {
      const dst = new Uint8Array(size);
      if (!ktx.transcodeImage(dst, 0, 0, 0, RGBA32, 0, -1, -1)) throw new Error('transcodeImage failed');
      return dst;
    };
    rgba = transcode();
    if (sha256(rgba) !== sha256(transcode())) throw new Error('two transcodes differ');
  } finally {
    ktx.close();
    ktx.delete();
  }

  // The plan's coverage, counted from the plan (the blocks are what it says they are).
  const modes = new Set(steps.map((s) => s.mode));
  const patterns = Object.fromEntries(Object.keys(PATTERNS).map((m) => [m, new Set()]));
  const selectors = Object.fromEntries(CCS_MODES.map((m) => [m, new Set()]));
  steps.forEach((s, i) => {
    const b = blocks[i];
    let v = 0n;
    for (let k = 15; k >= 0; k--) v = (v << 8n) | BigInt(b[k]);
    const at = MODE_CODES[s.mode][1] + HINT_BITS[s.mode];
    if (s.mode in PATTERNS) patterns[s.mode].add(Number((v >> BigInt(at)) & ((1n << BigInt(PATTERNS[s.mode][1])) - 1n)));
    if (CCS_MODES.includes(s.mode)) selectors[s.mode].add(Number((v >> BigInt(at)) & 3n));
  });
  for (let m = 0; m < 19; m++) if (!modes.has(m)) throw new Error(`mode ${m} is missing`);
  for (const [m, [count]] of Object.entries(PATTERNS)) {
    if (patterns[m].size !== count) throw new Error(`mode ${m} has ${patterns[m].size} of its ${count} patterns`);
  }
  for (const m of CCS_MODES) if (selectors[m].size !== 4) throw new Error(`mode ${m} has ${selectors[m].size} of 4 selectors`);

  fs.writeFileSync(path.join(outDir, `${NAME}.ktx2`), file);
  fs.writeFileSync(path.join(outDir, `${NAME}.rgba`), rgba);
  let translucent = 0;
  for (let i = 3; i < rgba.length; i += 4) if (rgba[i] !== 255) translucent++;
  const perMode = Array.from({ length: 19 }, (_, m) => steps.filter((s) => s.mode === m).length);
  console.log(
    JSON.stringify({
      file: `${NAME}.ktx2`,
      bytes: file.length,
      sha256: sha256(file),
      width: WIDTH,
      height: HEIGHT,
      blocks: blocks.length,
      blocksPerMode: perMode,
      translucentPixels: translucent,
      reference: `${NAME}.rgba`,
      referenceBytes: rgba.length,
      referenceSha256: sha256(rgba),
      node: process.version,
    }),
  );
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
