#!/usr/bin/env node
// Test vectors for litegen-core/src/mesh/meshopt.rs (mesh-gaps item 2, Task 1):
// meshopt streams this script builds by hand, and what meshoptimizer's own
// reference decoder reads from each, printed as the Rust consts that
// meshopt.rs's tests compare with.
//
//   node scripts/mesh-verify/meshopt-vectors.mjs <meshopt_decoder_reference.js>
//
// The argument is js/meshopt_decoder_reference.js from zeux/meshoptimizer at
// f0a684f11ead064e176775c9b796f82a55910e92 (MIT). It isn't kept in this repo:
//
//   curl -O https://raw.githubusercontent.com/zeux/meshoptimizer/f0a684f11ead064e176775c9b796f82a55910e92/js/meshopt_decoder_reference.js
//
// Its sha256 is checked before it runs (REFERENCE_SHA256). It is the only
// third-party code involved: the streams are ours. This script encodes them
// from data it chooses, following Appendix A of the KHR_meshopt_compression
// spec, and forces the encodings each test needs: every v0 header mode, every
// v1 control mode, both v1 bit tables, every channel mode, every triangle
// code, both index baselines. The reference decoder then decodes each stream.
// For ATTRIBUTES and INDICES the script checks that it reads back the data the
// stream was built from, so a mistake in this encoder can't become a test
// vector; for TRIANGLES it checks only that the 16-bit decode equals the 32-bit
// one, wrapped. It prints what the decoder read, which is what the tests
// expect. A summary of what each stream holds goes to stderr.
//
// The committed vectors were made with Node v26.9.0. The OCTAHEDRAL and
// QUATERNION inputs are normalised with Math.hypot, which ECMAScript defines
// as implementation-approximated, so another engine could change those inputs
// and, with them, the streams: a diff on regeneration under another Node may
// have no other cause.
//
// With --views it reads a glTF file's own streams instead (mesh-gaps item 2,
// Task 2): see "--views" below.
import fs from 'node:fs';
import crypto from 'node:crypto';
import path from 'node:path';

const REFERENCE_COMMIT = 'f0a684f11ead064e176775c9b796f82a55910e92';
const REFERENCE_SHA256 = '207f64595be3d5d0c0d728a981673c3fee8b4d9e671341831d556485d96de8dc';

const [referencePath, ...rest] = process.argv.slice(2);
if (!referencePath) {
  console.error('usage: node meshopt-vectors.mjs <meshopt_decoder_reference.js> [--views <file.gltf> <out.bin>]');
  process.exit(2);
}
const referenceSource = fs.readFileSync(referencePath);
const referenceSha = crypto.createHash('sha256').update(referenceSource).digest('hex');
if (referenceSha !== REFERENCE_SHA256) {
  console.error(`${referencePath}: sha256 ${referenceSha}, not ${REFERENCE_SHA256} (js/meshopt_decoder_reference.js at ${REFERENCE_COMMIT})`);
  process.exit(1);
}
// Imported from the bytes just hashed, so what runs is what was checked. The
// file is an ES module with no imports of its own.
const { MeshoptDecoder } = await import(`data:text/javascript;base64,${referenceSource.toString('base64')}`);

function fail(message) {
  throw new Error(message);
}

function same(a, b) {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

/** What the reference decoder reads from `stream`: count × stride bytes. */
function reference(mode, stream, count, stride, filter) {
  const target = new Uint8Array(count * stride);
  MeshoptDecoder.decodeGltfBuffer(target, count, stride, Uint8Array.from(stream), mode, filter);
  return target;
}

// ---------------------------------------------------------------------------
// --views: a glTF file's own compressed bufferViews, as the reference decoder
// reads them, for glb.rs's MeshoptCubeTest differential.
//
//   node scripts/mesh-verify/meshopt-vectors.mjs <meshopt_decoder_reference.js> --views <file.gltf> <out.bin>
//
// Every bufferView of <file.gltf> that carries KHR_meshopt_compression or
// EXT_meshopt_compression, in view order, is decoded from its extension's own
// byte range (the buffer's uri, a file beside the .gltf), filter applied.
// <out.bin> holds a record a view: the view's index and the decoded length as
// u32 little-endian, then the decoded bytes. A line a view, and the file's
// length and sha256, go to stderr.

if (rest[0] === '--views') {
  const [, gltfPath, outPath] = rest;
  if (!gltfPath || !outPath) {
    console.error('usage: node meshopt-vectors.mjs <meshopt_decoder_reference.js> --views <file.gltf> <out.bin>');
    process.exit(2);
  }
  const doc = JSON.parse(fs.readFileSync(gltfPath, 'utf8'));
  const buffers = new Map();
  const records = [];
  const lines = [];
  (doc.bufferViews ?? []).forEach((view, index) => {
    const extensions = view.extensions ?? {};
    const name = ['KHR_meshopt_compression', 'EXT_meshopt_compression'].find((n) => n in extensions);
    if (!name) return;
    const e = extensions[name];
    if (!buffers.has(e.buffer)) {
      const uri = doc.buffers?.[e.buffer]?.uri;
      if (!uri || uri.startsWith('data:')) fail(`bufferView ${index}: buffer ${e.buffer} is not a file beside the .gltf`);
      buffers.set(e.buffer, fs.readFileSync(path.join(path.dirname(gltfPath), decodeURIComponent(uri))));
    }
    const offset = e.byteOffset ?? 0;
    const stream = buffers.get(e.buffer).subarray(offset, offset + e.byteLength);
    if (stream.length !== e.byteLength) fail(`bufferView ${index}: its stream runs past buffer ${e.buffer}`);
    const filter = e.filter ?? 'NONE';
    const decoded = reference(e.mode, stream, e.count, e.byteStride, filter);
    const head = Buffer.alloc(8);
    head.writeUInt32LE(index, 0);
    head.writeUInt32LE(decoded.length, 4);
    records.push(head, Buffer.from(decoded));
    lines.push(`bufferView ${index}: ${name} ${e.mode} ${filter}, header 0x${stream[0].toString(16)}, ${e.count} × ${e.byteStride} bytes from a ${stream.length}-byte stream`);
  });
  const file = Buffer.concat(records);
  fs.writeFileSync(outPath, file);
  console.error(lines.join('\n'));
  console.error(`${lines.length} views, ${file.length} bytes, sha256 ${crypto.createHash('sha256').update(file).digest('hex')}`);
  process.exit(0);
}

/** xorshift32 (Marsaglia, 2003): the same data every run. */
function prng(seed) {
  let s = seed >>> 0;
  return () => {
    s ^= s << 13;
    s >>>= 0;
    s ^= s >>> 17;
    s ^= s << 5;
    s >>>= 0;
    return s;
  };
}

// ---------------------------------------------------------------------------
// Mode 0, ATTRIBUTES (Appendix A, "Mode 0: attributes")

const V0_BITS = [0, 2, 4, 8];
const V1_BITS = [[0, 1, 2, 4], [1, 2, 4, 8]]; // control mode 0, control mode 1

const maxBlockElements = (stride) => Math.min(Math.floor(8192 / stride) & ~15, 256);
const zz8 = (diff) => {
  const s = (diff << 24) >> 24;
  return ((s << 1) ^ (s >> 31)) & 0xff;
};
const zz16 = (diff) => {
  const s = (diff << 16) >> 16;
  return ((s << 1) ^ (s >> 31)) & 0xffff;
};
const rotl = (x, r) => ((x << r) | (x >>> ((32 - r) & 31))) >>> 0;
const u32At = (a, i) => (a[i] | (a[i + 1] << 8) | (a[i + 2] << 16) | (a[i + 3] << 24)) >>> 0;

/** Encoded size of one 16-delta group at `bits` per delta. */
function groupSize(vals, bits) {
  if (bits === 0) return vals.every((v) => v === 0) ? 0 : Infinity;
  if (bits === 8) return 16;
  const sentinel = (1 << bits) - 1;
  return 2 * bits + vals.filter((v) => v >= sentinel).length;
}

function encodeGroup(vals, bits, out) {
  if (bits === 0) {
    if (!vals.every((v) => v === 0)) fail('a 0-bit group with a delta that is not 0');
    return;
  }
  if (bits === 8) {
    out.push(...vals);
    return;
  }
  const sentinel = (1 << bits) - 1;
  const packed = new Array(2 * bits).fill(0);
  const extra = [];
  for (let m = 0; m < 16; m++) {
    const v = vals[m] >= sentinel ? sentinel : vals[m];
    if (v === sentinel) extra.push(vals[m]);
    if (bits === 1) packed[m >> 3] |= v << (m & 7); // least significant bit first
    else if (bits === 2) packed[m >> 2] |= v << (6 - 2 * (m & 3)); // most significant first
    else packed[m >> 1] |= v << (4 - 4 * (m & 1));
  }
  out.push(...packed, ...extra);
}

/** One byte lane of one block, header bits then groups: the given modes, or the smallest. */
function encodeLane(deltas, table, forced) {
  const groups = Math.ceil(deltas.length / 16);
  const vals = (g) => Array.from({ length: 16 }, (_, m) => deltas[g * 16 + m] ?? 0);
  const modes = [];
  for (let g = 0; g < groups; g++) {
    if (forced?.[g] !== undefined) {
      if (groupSize(vals(g), table[forced[g]]) === Infinity) fail(`group ${g} can't take mode ${forced[g]}`);
      modes.push(forced[g]);
      continue;
    }
    let best = 0;
    for (let m = 1; m < 4; m++) if (groupSize(vals(g), table[m]) < groupSize(vals(g), table[best])) best = m;
    modes.push(best);
  }
  const header = new Array(Math.ceil(groups / 4)).fill(0);
  modes.forEach((m, g) => (header[g >> 2] |= m << ((g & 3) * 2)));
  const body = [];
  modes.forEach((m, g) => encodeGroup(vals(g), table[m], body));
  return { bytes: [...header, ...body], modes };
}

/**
 * An ATTRIBUTES stream of `data` (count × stride bytes). `force(block, lane)`
 * may return `{ ctrl, modes }` for a lane: its v1 control mode and its groups'
 * header modes. Everything not forced takes the smallest encoding.
 */
function encodeAttributes({ version, stride, data, baseline, channels = [], force = () => ({}) }) {
  if (baseline.length !== stride || data.length % stride) fail('the data is not whole elements');
  if (version === 1 && channels.length !== stride / 4) fail('one channel mode per 4 bytes');
  const count = data.length / stride;
  const maxBlock = maxBlockElements(stride);
  const out = [version === 0 ? 0xa0 : 0xa1];
  const prev = Uint8Array.from(baseline);
  const shape = [];
  for (let start = 0, block = 0; start < count; start += maxBlock, block++) {
    const n = Math.min(count - start, maxBlock);
    const lanes = Array.from({ length: stride }, () => new Array(n).fill(0));
    for (let i = 0; i < n; i++) {
      const cur = data.subarray((start + i) * stride, (start + i + 1) * stride);
      for (let ch = 0; ch < stride / 4; ch++) {
        const mode = version === 0 ? 0 : channels[ch] & 15;
        const b = ch * 4;
        if (mode === 0) {
          for (let k = b; k < b + 4; k++) lanes[k][i] = zz8(cur[k] - prev[k]);
        } else if (mode === 1) {
          for (let k = b; k < b + 4; k += 2) {
            const d = zz16((cur[k] | (cur[k + 1] << 8)) - (prev[k] | (prev[k + 1] << 8)));
            lanes[k][i] = d & 0xff;
            lanes[k + 1][i] = d >> 8;
          }
        } else {
          const d = rotl((u32At(cur, b) ^ u32At(prev, b)) >>> 0, channels[ch] >> 4);
          for (let j = 0; j < 4; j++) lanes[b + j][i] = (d >>> (8 * j)) & 0xff;
        }
      }
      prev.set(cur);
    }
    const control = new Array(version === 0 ? 0 : stride / 4).fill(0);
    const body = [];
    const lanesShape = [];
    for (let lane = 0; lane < stride; lane++) {
      const want = force(block, lane) ?? {};
      const deltas = lanes[lane];
      let ctrl = 0;
      let encoded;
      if (version === 0) {
        encoded = encodeLane(deltas, V0_BITS, want.modes);
      } else {
        const options = [
          encodeLane(deltas, V1_BITS[0], want.ctrl === 0 ? want.modes : undefined),
          encodeLane(deltas, V1_BITS[1], want.ctrl === 1 ? want.modes : undefined),
          deltas.every((v) => v === 0) ? { bytes: [], modes: [] } : null,
          { bytes: deltas, modes: [] },
        ];
        if (want.ctrl !== undefined) {
          if (!options[want.ctrl]) fail(`lane ${lane} of block ${block} can't take control mode ${want.ctrl}`);
          ctrl = want.ctrl;
        } else {
          for (let c = 1; c < 4; c++) if (options[c] && options[c].bytes.length < options[ctrl].bytes.length) ctrl = c;
        }
        encoded = options[ctrl];
        control[lane >> 2] |= ctrl << ((lane & 3) * 2);
      }
      body.push(...encoded.bytes);
      lanesShape.push({ ctrl, modes: encoded.modes });
    }
    out.push(...control, ...body);
    shape.push({ n, lanes: lanesShape });
  }
  const tailSize = stride + (version === 0 ? 0 : stride / 4);
  for (let i = tailSize; i < (version === 0 ? 32 : 24); i++) out.push(0);
  out.push(...baseline);
  if (version === 1) out.push(...channels);
  const stream = Uint8Array.from(out);
  if (!same(reference('ATTRIBUTES', stream, count, stride), data)) fail('the reference decoder reads something else from this stream');
  return { stream, count, stride, shape };
}

/** A lane's values from a start value and its zig-zag byte deltas. */
function fromDeltas(start, zigzags) {
  const out = [];
  let v = start;
  for (const z of zigzags) {
    v = (v + ((z >> 1) ^ -(z & 1))) & 0xff;
    out.push(v);
  }
  return out;
}

function describeAttributes(name, { stream, count, stride, shape }) {
  const lines = [`${name}: v${stream[0] & 15}, ${count} × ${stride} bytes, ${shape.length} block(s) of ${shape.map((b) => b.n).join(' + ')}, ${stream.length}-byte stream`];
  shape.forEach((b, i) => {
    const ctrls = b.lanes.map((l) => l.ctrl).join('');
    lines.push(`  block ${i}: control modes ${ctrls}; lane header modes ${b.lanes.map((l) => l.modes.join('') || '-').join(' ')}`);
  });
  return lines.join('\n');
}

// ---------------------------------------------------------------------------
// Mode 1, TRIANGLES (Appendix A, "Mode 1: triangles")

/**
 * Triangle codes one at a time, with the data each needs. It refuses a code
 * that reads an edge or vertex FIFO entry no earlier code wrote, which the
 * spec makes invalid; meshopt.rs refuses such streams, and the reference
 * decoder would read its zero-filled FIFOs instead.
 */
function triangleBuilder(table) {
  if (table.length !== 16 || table[14] || table[15] || table.some((e) => e >> 4 === 15 || (e & 15) === 15)) fail('an invalid code table');
  const codes = [];
  const data = [];
  let last = 0;
  let edges = 0;
  let verts = 0;
  const pushV = (k) => (verts = Math.min(16, verts + k));
  const pushE = (k) => (edges = Math.min(16, edges + k));
  const readE = (x) => x < edges || fail(`edge FIFO entry ${x}, with ${edges} written`);
  const readV = (y) => y < verts || fail(`vertex FIFO entry ${y}, with ${verts} written`);
  const varint = (v) => {
    v >>>= 0;
    do {
      data.push((v & 127) | (v > 127 ? 128 : 0));
      v >>>= 7;
    } while (v);
  };
  const free = (index) => {
    const d = (index - last) | 0;
    varint(((d << 1) ^ (d >> 31)) >>> 0);
    last = index >>> 0;
  };
  const edgeCode = (x, low) => {
    if (x > 14) fail('edge FIFO entries go to 14');
    readE(x);
    codes.push((x << 4) | low);
  };
  return {
    /** 0xX0: edge X, and the next new vertex. */
    edgeNext(x) {
      edgeCode(x, 0);
      pushV(1);
      pushE(2);
    },
    /** 0xXY, 0 < Y < 13: edge X, and vertex FIFO entry Y. */
    edgeVertex(x, y) {
      if (y < 1 || y > 12) fail('0xXY takes Y in 1..=12');
      readV(y);
      edgeCode(x, y);
      pushE(2);
    },
    /** 0xXd: edge X, and last - 1. */
    edgeDec(x) {
      edgeCode(x, 13);
      last = (last - 1) >>> 0;
      pushV(1);
      pushE(2);
    },
    /** 0xXe: edge X, and last + 1. */
    edgeInc(x) {
      edgeCode(x, 14);
      last = (last + 1) >>> 0;
      pushV(1);
      pushE(2);
    },
    /** 0xXf: edge X, and an index coded as a delta from last. */
    edgeFree(x, index) {
      edgeCode(x, 15);
      free(index);
      pushV(1);
      pushE(2);
    },
    /** 0xfY, Y < 14: the code table's entry Y. */
    aux(y) {
      if (y > 13) fail('0xfY takes Y in 0..=13');
      const z = table[y] >> 4;
      const w = table[y] & 15;
      if (z) readV(z - 1);
      if (w) readV(w - 1);
      codes.push(0xf0 | y);
      pushV(1 + (z === 0) + (w === 0));
      pushE(3);
    },
    /** 0xfe or 0xff, with its byte 0xZW and the indices it codes explicitly. */
    explicit(code, zw, { a, b, c } = {}) {
      const z = zw >> 4;
      const w = zw & 15;
      if (z > 0 && z < 15) readV(z - 1);
      if (w > 0 && w < 15) readV(w - 1);
      codes.push(code);
      data.push(zw);
      if (code === 0xff) free(a);
      if (z === 15) free(b);
      if (w === 15) free(c);
      pushV(1 + (z === 0 || z === 15) + (w === 0 || w === 15));
      pushE(3);
    },
    build() {
      return { stream: Uint8Array.from([0xe1, ...codes, ...data, ...table]), count: codes.length * 3, codes };
    },
  };
}

// ---------------------------------------------------------------------------
// Mode 2, INDICES (Appendix A, "Mode 2: indices")

function indexSequence(steps) {
  const out = [0xd1];
  const last = [0, 0];
  const values = [];
  for (const [baseline, delta] of steps) {
    if (delta < -(2 ** 30) || delta >= 2 ** 30) fail('deltas are limited to [-2^30, 2^30)');
    let v = (((delta << 1) ^ (delta >> 31)) >>> 0) * 2 + baseline;
    do {
      out.push((v & 127) | (v > 127 ? 128 : 0));
      v = Math.floor(v / 128);
    } while (v);
    last[baseline] = (last[baseline] + delta) >>> 0;
    values.push(last[baseline]);
  }
  out.push(0, 0, 0, 0);
  return { stream: Uint8Array.from(out), values };
}

// ---------------------------------------------------------------------------
// Rust output

const hex = (v, width) => `0x${v.toString(16).padStart(width, '0')}`;

function rust(name, type, values, perLine, format) {
  const rows = [];
  for (let i = 0; i < values.length; i += perLine) rows.push(`    ${Array.from(values.slice(i, i + perLine), format).join(', ')},`);
  return `const ${name}: &[${type}] = &[\n${rows.join('\n')}\n];`;
}
const bytes = (name, values) => rust(name, 'u8', values, 16, (v) => hex(v, 2));

function fnv1a64(data) {
  let h = 0xcbf29ce484222325n;
  for (const b of data) h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  return h;
}

const sections = [];
const summary = [];
const emit = (doc, ...items) => sections.push([...doc.split('\n').map((l) => `// ${l}`), ...items].join('\n'));

// ---------------------------------------------------------------------------
// attributes_v0_decode: v0, stride 4, 260 elements, so blocks of 256 and 4

{
  const random = prng(0x5eed0001);
  const count = 260;
  const stride = 4;
  const baseline = [0x10, 0x80, 0xff, 0x00];
  // Lane 0 as zig-zag deltas, group by group.
  const lane0Deltas = [
    ...new Array(16).fill(0), // group 0, header mode 0: every delta 0
    ...[0, 1, 2, 3, 0, 2, 1, 7, 0, 0, 2, 200, 1, 1, 0, 2], // group 1, 2 bits: 3, 7 and 200 are escaped
    ...[5, 14, 15, 0, 9, 30, 1, 14, 255, 3, 2, 8, 13, 16, 0, 7], // group 2, 4 bits: 15, 30, 255 and 16 are escaped
    ...Array.from({ length: 16 }, () => random() & 0xff), // group 3, 8 bits
  ];
  const lane0Values = fromDeltas(baseline[0], lane0Deltas);
  for (let g = 4; g < 16; g++) {
    const kind = g % 4;
    const deltas = Array.from({ length: 16 }, () => (kind === 0 ? 0 : kind === 1 ? random() % 4 : kind === 2 ? random() % 20 : random() & 0xff));
    lane0Values.push(...fromDeltas(lane0Values[lane0Values.length - 1], deltas));
  }
  lane0Values.push(...fromDeltas(lane0Values[lane0Values.length - 1], [3, 0, 255, 1]));
  const data = new Uint8Array(count * stride);
  for (let i = 0; i < count; i++) {
    data[i * 4 + 0] = lane0Values[i];
    data[i * 4 + 1] = (0x80 + i * 3 + (i >> 4) * 17) & 0xff; // a ramp that jumps every group
    data[i * 4 + 2] = i & 1 ? 0x00 : 0xff; // wraps around 0xff ↔ 0x00
    data[i * 4 + 3] = random() & 0xff;
  }
  const v = encodeAttributes({ version: 0, stride, data, baseline, force: (block, lane) => (block === 0 && lane === 0 ? { modes: [0, 1, 2, 3] } : {}) });
  summary.push(describeAttributes('ATTR_V0', v));
  emit(
    'attributes_v0_decode: v0, byteStride 4, 260 elements (blocks of 256 and 4). Lane 0 of\nblock 0 takes header modes 0-3 in its first four groups, with escapes in the\n2- and 4-bit groups; the tail is padded to 32 bytes.',
    bytes('ATTR_V0_STREAM', v.stream),
    bytes('ATTR_V0_DECODED', reference('ATTRIBUTES', v.stream, count, stride)),
  );
}

// ---------------------------------------------------------------------------
// attributes_v1_control_modes_decode, first stream: v1, stride 16, 72
// elements in one block; every control mode, both bit tables, every channel mode

{
  const random = prng(0x5eed0002);
  const count = 72;
  const stride = 16;
  const channels = [0x00, 0x01, 0x52, 0xf2]; // byte deltas; 2-byte deltas; XOR rotated by 5; XOR rotated by 15
  const baseline = Array.from({ length: 16 }, (_, k) => (k * 17 + 3) & 0xff);
  // lane 0 (control 0, bits 0/1/2/4) and lane 1 (control 1, bits 1/2/4/8) as zig-zag deltas, group by group
  const lane0 = [
    ...new Array(16).fill(0), // 0 bits
    ...[0, 0, 5, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 130, 0, 0], // 1 bit: 5, 1 and 130 are escaped
    ...[2, 1, 0, 3, 2, 2, 1, 0, 9, 0, 1, 2, 0, 0, 1, 2], // 2 bits: 3 and 9 are escaped
    ...[14, 3, 15, 7, 0, 1, 2, 3, 4, 5, 6, 7, 8, 200, 10, 11], // 4 bits: 15 and 200 are escaped
    ...[0, 0, 0, 0, 0, 0, 0, 1], // 1 bit, the last 8 elements: 1 is escaped
  ];
  const lane1 = [
    ...[0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // 1 bit
    ...[1, 2, 0, 0, 1, 2, 4, 0, 1, 1, 2, 2, 0, 0, 1, 0], // 2 bits
    ...[9, 3, 15, 0, 1, 12, 14, 2, 3, 30, 1, 2, 5, 6, 7, 8], // 4 bits
    ...Array.from({ length: 16 }, () => random() & 0xff), // 8 bits
    ...[0, 0, 0, 0, 0, 0, 0, 0], // 1 bit
  ];
  const v0 = fromDeltas(baseline[0], lane0);
  const v1 = fromDeltas(baseline[1], lane1);
  const data = new Uint8Array(count * stride);
  for (let i = 0; i < count; i++) {
    const e = data.subarray(i * 16, i * 16 + 16);
    e[0] = v0[i];
    e[1] = v1[i];
    e[2] = baseline[2]; // control mode 2: every delta 0
    e[3] = random() & 0xff; // control mode 3: literal
    const a = (1000 + i * 37 + (i >= 40 ? 3000 : 0)) & 0xffff; // channel 1: two 16-bit values
    const b = random() & 0xffff;
    e[4] = a & 0xff;
    e[5] = a >> 8;
    e[6] = b & 0xff;
    e[7] = b >> 8;
    const x = (0x3f800000 + i * 0x00010000) >>> 0; // channel 2: float-like bits
    const y = Math.imul(i + 1, 0x01010101) >>> 0; // channel 3
    for (let j = 0; j < 4; j++) {
      e[8 + j] = (x >>> (8 * j)) & 0xff;
      e[12 + j] = (y >>> (8 * j)) & 0xff;
    }
  }
  const force = (block, lane) => ({ 0: { ctrl: 0, modes: [0, 1, 2, 3, 1] }, 1: { ctrl: 1, modes: [0, 1, 2, 3, 0] }, 2: { ctrl: 2 }, 3: { ctrl: 3 } })[lane] ?? {};
  const v = encodeAttributes({ version: 1, stride, data, baseline, channels, force });
  summary.push(describeAttributes('ATTR_V1', v));
  emit(
    'attributes_v1_control_modes_decode: v1, byteStride 16, 72 elements in one block.\nControl modes: lane 0 takes 0 (header modes 0-3: 0, 1, 2 and 4 bits), lane 1\ntakes 1 (header modes 0-3: 1, 2, 4 and 8 bits), lane 2 takes 2, lane 3 takes 3.\nChannel modes 0, 1, 2 (rotation 5) and 2 (rotation 15). The tail (20 bytes) is\npadded to 24.',
    bytes('ATTR_V1_STREAM', v.stream),
    bytes('ATTR_V1_DECODED', reference('ATTRIBUTES', v.stream, count, stride)),
  );
}

// ---------------------------------------------------------------------------
// attributes_v1_control_modes_decode, second stream: v1, stride 48, so blocks
// of 160 elements ((8192 / 48) & ~15), and 162 of them: blocks of 160 and 2

{
  const count = 162;
  const stride = 48;
  const channels = [0x00, 0x01, 0x32, 0x00, 0x01, 0x72, 0x00, 0x01, 0xa2, 0x00, 0x01, 0x02];
  // Channels 0-2 (one of each channel mode) change every element; the rest
  // are constant, which control mode 2 stores in no bytes, except for a byte
  // that changes in block 1 only, so the two blocks' control headers differ.
  const data = new Uint8Array(count * stride);
  for (let i = 0; i < count; i++) {
    for (let c = 0; c < 12; c++) {
      const e = data.subarray(i * stride + c * 4, i * stride + c * 4 + 4);
      if (c === 0) {
        e.set([(i + 7) & 0xff, (i * 2 + 1) & 0xff, 0x11, i >= 160 ? 9 : 0]);
      } else if (c === 1) {
        const a = 1000 + 5 * i;
        const b = 40000 - 3 * i;
        e.set([a & 0xff, a >> 8, b & 0xff, b >> 8]);
      } else if (c === 2) {
        const x = (0x40000000 + i * 0x00004000) >>> 0;
        for (let j = 0; j < 4; j++) e[j] = (x >>> (8 * j)) & 0xff;
      } else {
        e.set([c, (c * 3) & 0xff, (c * 5) & 0xff, i >= 160 && c % 3 === 0 ? 1 : 0]);
      }
    }
  }
  const baseline = Array.from(data.subarray(0, stride));
  const v = encodeAttributes({ version: 1, stride, data, baseline, channels });
  if (maxBlockElements(stride) !== 160 || v.shape.length !== 2) fail('the wide stream is not two blocks of 160 and 2');
  summary.push(describeAttributes('ATTR_WIDE', v));
  const decoded = reference('ATTRIBUTES', v.stream, count, stride);
  emit(
    `attributes_v1_control_modes_decode, second stream: v1, byteStride 48, 162 elements:\nblocks of 160 ((8192 / 48) & ~15) and 2, each with its own control header.\nIts ${decoded.length} decoded bytes are pinned by their FNV-1a 64 and by elements 158-161,\nthe two either side of the block boundary.`,
    bytes('ATTR_WIDE_STREAM', v.stream),
    `const ATTR_WIDE_DECODED_FNV1A: u64 = 0x${fnv1a64(decoded).toString(16).padStart(16, '0')};`,
    bytes('ATTR_WIDE_DECODED_158_TO_161', decoded.subarray(158 * stride, 162 * stride)),
  );
}

// ---------------------------------------------------------------------------
// triangles_decode

{
  // A code table of our own (meshoptimizer's encoder writes another), so a
  // decoder that ignores the stream's table and uses meshoptimizer's fails.
  const table = [0x00, 0x01, 0x10, 0x12, 0x21, 0x34, 0x43, 0x56, 0x65, 0x78, 0x87, 0x9a, 0xbc, 0xde, 0x00, 0x00];
  const t = triangleBuilder(table);
  t.aux(0); // 0xf0: three new vertices
  t.edgeNext(0);
  t.edgeNext(2);
  t.edgeVertex(1, 1);
  t.edgeFree(0, 70000); // past 65535: 16-bit output wraps
  t.edgeInc(1);
  t.edgeDec(0);
  t.edgeFree(2, 4); // a negative delta
  t.aux(1);
  t.aux(2);
  t.explicit(0xfe, 0x0f, { c: 12345 });
  t.explicit(0xff, 0x23, { a: 7 });
  t.edgeVertex(14, 12); // the oldest edge, and vertex entry 12
  t.aux(13); // vertex entries 12 and 13
  t.aux(7);
  t.explicit(0xfe, 0xf0, { b: 100000 });
  t.explicit(0xff, 0xff, { a: 3, b: 99999, c: 65536 });
  t.explicit(0xfe, 0x9e, {});
  t.edgeNext(7);
  t.edgeVertex(3, 5);
  t.edgeInc(14);
  t.edgeDec(5);
  t.explicit(0xff, 0x00, { a: 5 }); // 0xZW = 0 resets next
  t.edgeNext(0);
  t.aux(12);
  t.edgeVertex(9, 2);
  t.aux(4);
  t.explicit(0xfe, 0x00, {}); // last: resets next, then codes 0, 1, 2
  const { stream, count, codes } = t.build();
  const u32 = new Uint32Array(reference('TRIANGLES', stream, count, 4).buffer);
  const u16 = new Uint16Array(reference('TRIANGLES', stream, count, 2).buffer);
  if (!same(Array.from(u16), Array.from(u32, (v) => v & 0xffff))) fail('the 16-bit triangles are not the 32-bit ones, wrapped');
  summary.push(`TRIANGLES: ${codes.length} triangles, codes ${codes.map((c) => hex(c, 2)).join(' ')}, ${stream.length}-byte stream`);
  emit(
    `triangles_decode: ${codes.length} triangles of every code: 0xX0, 0xXY, 0xXd, 0xXe, 0xXf,\n0xfY (our own code table), 0xfe and 0xff, with resets (0xZW = 0) by 0xff (present,\nnot asserted) and, last and asserted, by 0xfe. Read at byteStride 4 and 2.`,
    bytes('TRIANGLES_STREAM', stream),
    rust('TRIANGLES_DECODED_U32', 'u32', Array.from(u32), 12, String),
    rust('TRIANGLES_DECODED_U16', 'u16', Array.from(u16), 12, String),
  );
}

// ---------------------------------------------------------------------------
// index_sequence_decodes

{
  const steps = [
    [0, 5], [1, 100], [0, 1], [0, -3], [1, 70000], [1, -1], [0, 0], [0, -3],
    [1, -70099], [0, 2 ** 30 - 1], [0, -(2 ** 30)], [0, 1], [1, 2], [1, 3], [0, 60000], [0, -59999],
  ];
  const { stream, values } = indexSequence(steps);
  const u32 = new Uint32Array(reference('INDICES', stream, values.length, 4).buffer);
  const u16 = new Uint16Array(reference('INDICES', stream, values.length, 2).buffer);
  if (!same(Array.from(u32), values)) fail('the reference decoder reads other indices');
  if (!same(Array.from(u16), values.map((v) => v & 0xffff))) fail('the 16-bit indices are not the 32-bit ones, wrapped');
  summary.push(`INDICES: ${values.length} indices ${values.join(' ')}, ${stream.length}-byte stream`);
  emit(
    'index_sequence_decodes: 16 indices on both baselines, with negative deltas, the\nlargest deltas either way (2^30 - 1 and -2^30, five-byte varints) and u32\nwraparound. Read at byteStride 4 and 2.',
    bytes('INDICES_STREAM', stream),
    rust('INDICES_DECODED_U32', 'u32', Array.from(u32), 8, String),
    rust('INDICES_DECODED_U16', 'u16', Array.from(u16), 8, String),
  );
}

// ---------------------------------------------------------------------------
// Filters (Appendix B). Each stream is ATTRIBUTES; the filter is applied after.

function filtered(name, { version, stride, raw, channels, filter, Typed, perLine, format, doc }) {
  const data = new Uint8Array(raw.buffer.slice(0));
  const baseline = Array.from(data.subarray(0, stride));
  const v = encodeAttributes({ version, stride, data, baseline, channels });
  const count = data.length / stride;
  const out = new Typed(reference('ATTRIBUTES', v.stream, count, stride, filter).buffer);
  const typeName = { Int8Array: 'i8', Int16Array: 'i16', Uint8Array: 'u8', Uint16Array: 'u16', Uint32Array: 'u32' }[Typed.name];
  summary.push(describeAttributes(name, v));
  emit(doc, bytes(`${name}_STREAM`, v.stream), rust(`${name}_RAW`, typeName, Array.from(new Typed(raw.buffer)), perLine, format), rust(`${name}_FILTERED`, typeName, Array.from(out), perLine, format));
}

/** Unit vectors to octahedral K-bit (meshoptimizer's encoding): x, y, then 1.0 at K bits. */
function octahedral(n, k) {
  const max = (1 << (k - 1)) - 1;
  const l1 = Math.abs(n[0]) + Math.abs(n[1]) + Math.abs(n[2]);
  let x = n[0] / l1;
  let y = n[1] / l1;
  if (n[2] < 0) {
    const ox = x;
    x = (1 - Math.abs(y)) * (x >= 0 ? 1 : -1);
    y = (1 - Math.abs(ox)) * (y >= 0 ? 1 : -1);
  }
  return [Math.round(x * max), Math.round(y * max), max];
}

function unitVectors(seed, count) {
  const random = prng(seed);
  const fixed = [[0, 0, 1], [0, 0, -1], [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [1, 1, 1], [-1, 1, -1]];
  const out = fixed.map((v) => v.map((c) => c / Math.hypot(...v)));
  while (out.length < count) {
    const v = [0, 1, 2].map(() => (random() / 2 ** 32) * 2 - 1);
    const l = Math.hypot(...v);
    if (l > 0.1 && l <= 1) out.push(v.map((c) => c / l));
  }
  return out;
}

{
  const ks = [8, 7, 6, 5, 4, 3, 2, 8];
  const extra = [127, -127, 0, 1, -128, 55, -3, 100];
  const raw = new Int8Array(16 * 4);
  unitVectors(0x5eed0003, 16).forEach((n, i) => {
    raw.set([...octahedral(n, ks[i % ks.length]), extra[i % extra.length]], i * 4);
  });
  filtered('OCT8', {
    version: 0, stride: 4, raw, filter: 'OCTAHEDRAL', Typed: Int8Array, perLine: 16, format: String,
    doc: 'octahedral_filter_8_and_16_bit, 8 bits: 16 unit vectors (the six axes, two\ndiagonals and eight random ones, half of them below z = 0) at K = 2..8, with a\nfourth component the filter passes through. v0.',
  });
}

{
  const ks = [16, 12, 10, 8, 14, 15, 9, 2];
  const extra = [32767, -32767, 0, 1, -32768, 12345, -3, 1000];
  const raw = new Int16Array(16 * 4);
  unitVectors(0x5eed0004, 16).forEach((n, i) => {
    raw.set([...octahedral(n, ks[i % ks.length]), extra[i % extra.length]], i * 4);
  });
  filtered('OCT16', {
    version: 1, stride: 8, raw, channels: [0x01, 0x01], filter: 'OCTAHEDRAL', Typed: Int16Array, perLine: 8, format: String,
    doc: 'octahedral_filter_8_and_16_bit, 16 bits: 16 unit vectors at K from 2 to 16,\nsampled (2, 8, 9, 10, 12, 14, 15, 16; no test asserts K), with a\nfourth component the filter passes through. v1, 2-byte deltas.',
  });
}

{
  // Unit quaternions to meshoptimizer's encoding: the three smallest
  // components (largest made positive), scaled by sqrt(2), in K bits, after it
  // in cyclic order; then 1.0 at K bits with the largest's index in the low bits.
  const random = prng(0x5eed0005);
  const ks = [16, 12, 10, 8, 6, 4, 15, 11];
  const quats = [[0, 0, 0, 1], [1, 0, 0, 0], [0, -1, 0, 0], [0, 0, 0.6, -0.8]];
  while (quats.length < 16) {
    const q = [0, 1, 2, 3].map(() => (random() / 2 ** 32) * 2 - 1);
    const l = Math.hypot(...q);
    if (l > 0.1 && l <= 1) quats.push(q.map((c) => c / l));
  }
  const raw = new Int16Array(16 * 4);
  quats.forEach((q, i) => {
    const k = ks[i % ks.length];
    const max = (1 << (k - 1)) - 1;
    let qc = 0;
    for (let j = 1; j < 4; j++) if (Math.abs(q[j]) > Math.abs(q[qc])) qc = j;
    const sign = q[qc] < 0 ? -1 : 1;
    const c = [1, 2, 3].map((j) => Math.round(q[(qc + j) % 4] * sign * Math.SQRT2 * max));
    raw.set([...c, (max & ~3) | qc], i * 4);
  });
  filtered('QUAT', {
    version: 1, stride: 8, raw, channels: [0x01, 0x01], filter: 'QUATERNION', Typed: Int16Array, perLine: 8, format: String,
    doc: 'quaternion_filter: 16 unit quaternions (identity, three axis-aligned ones and\ntwelve random ones; every largest-component index 0-3) at K from 4 to 16,\nsampled (4, 6, 8, 10, 11, 12, 15, 16; no test asserts K). v1.',
  });
}

{
  const pairs = [
    [0, 0], [0, 1], [0, -1], [0, 8388607], [0, -8388608], [-23, 8388607], [-100, 1], [-100, -8388608],
    [100, 1], [100, -8388607], [1, 3], [-1, 3], [-8, 1234567], [7, -7654321], [-30, 5000000], [30, 0],
    [12, 4194304], [-12, -4194305], [64, 99], [-64, -99], [5, 1], [-5, -1], [99, 8388607], [-99, 2],
    [42, -42], [-42, 42], [3, 6291456], [-127 + 27, 700000], [16, -1], [-16, 1], [0, 4096], [2, -8388608],
    [-3, 1048575], [9, -1048576], [-20, 3355443], [20, -3355443], [77, 123456], [-77, -123456], [50, 8000000], [-50, -8000000],
    [1, 0], [-1, 0], [10, 1], [-10, -1], [24, 7], [-24, -7], [8, 8388000], [-8, -8388000],
  ];
  const raw = new Uint32Array(pairs.map(([e, m]) => ((e << 24) | (m & 0xffffff)) >>> 0));
  filtered('EXP', {
    version: 0, stride: 12, raw, filter: 'EXPONENTIAL', Typed: Uint32Array, perLine: 6, format: (v) => hex(v, 8),
    doc: 'exponential_filter_is_exact: 16 elements of three values (byteStride 12), each\nan exponent in [-100, 100] over a 24-bit mantissa, the extremes of both\nincluded. The filtered values are f32 bits. v0.',
  });
}

/** YCoCg inputs at K bits whose RGB fits K bits, with alpha's top bit at K - 1. */
function colors(seed, ks) {
  const random = prng(seed);
  const out = [];
  for (let i = 0; i < 16; i++) {
    const k = ks[i % ks.length];
    const max = (1 << (k - 1)) - 1;
    const top = (1 << k) - 1;
    for (;;) {
      const y = random() % (top + 1);
      const co = (random() % (2 * max + 1)) - max;
      const cg = (random() % (2 * max + 1)) - max;
      const rgb = [y + co - cg, y + cg, y - co - cg];
      if (rgb.every((c) => c >= 0 && c <= top)) {
        out.push([y, co, cg, (1 << (k - 1)) | (random() % (1 << (k - 1)))]);
        break;
      }
    }
  }
  return out;
}

{
  const raw = new Uint8Array(16 * 4);
  colors(0x5eed0006, [8, 7, 6, 5, 4, 3, 2, 8]).forEach((c, i) => raw.set(c.map((v) => v & 0xff), i * 4));
  filtered('COLOR8', {
    version: 1, stride: 4, raw, channels: [0x00], filter: 'COLOR', Typed: Uint8Array, perLine: 16, format: String,
    doc: 'color_filter_ycocg, 8 bits: 16 colours at K = 2..8 (Y, Co, Cg, then alpha in\nK - 1 bits under its top bit). v1.',
  });
}

{
  const raw = new Uint16Array(16 * 4);
  colors(0x5eed0007, [16, 12, 10, 8, 15, 9, 4, 2]).forEach((c, i) => raw.set(c.map((v) => v & 0xffff), i * 4));
  filtered('COLOR16', {
    version: 0, stride: 8, raw, filter: 'COLOR', Typed: Uint16Array, perLine: 8, format: String,
    doc: 'color_filter_ycocg, 16 bits: 16 colours at K from 2 to 16, sampled (2, 4,\n8, 9, 10, 12, 15, 16; no test asserts K). v0.',
  });
}

console.error(summary.join('\n'));
console.log(
  [
    `// Generated by scripts/mesh-verify/meshopt-vectors.mjs: the streams are hand-built`,
    `// there, and every expected value is what meshoptimizer's reference decoder`,
    `// (js/meshopt_decoder_reference.js at ${REFERENCE_COMMIT},`,
    `// sha256 ${REFERENCE_SHA256}) reads from them:`,
    `//   node scripts/mesh-verify/meshopt-vectors.mjs <meshopt_decoder_reference.js>`,
    '',
    sections.join('\n\n'),
  ].join('\n'),
);
