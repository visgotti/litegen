#!/usr/bin/env node
// make_corpus.mjs: writes litegen's Zstandard test corpus, the files in this folder, with the
// `zstd` command line tool (libzstd, the reference implementation).
//
// What the corpus is for: ktx2.rs inflates Zstandard-supercompressed KTX2 levels with the `ruzstd`
// crate (mesh-gaps item 3, decision D3: "fully tested"). The tests decode every file here with it
// and require the bytes `zstd -d` gave. Every file is made, and every expected output is produced,
// by the reference tool, from inputs that are ours:
//
//   uastc_normal     level 0 of FlightHelmet_Materials_LensesMat_Normal.ktx2, the one UASTC level
//                    (1 MiB, CC0), inflated by `zstd -d` from the KTX2 file's own frame
//   uastc_all_modes  level 0 of uastc_all_modes.ktx2: 240 UASTC blocks of every mode (ours)
//   rle              4,096 bytes of 0xA5 (not 0: a decoder that left its buffer zeroed must fail)
//   random           4,096 bytes of a xorshift64 stream, seed 0xC0FFEE1234567890, each output as 8
//                    little-endian bytes: incompressible, so raw blocks
//   structured       64 fixed-width lines of a made-up job log (fields drawn from a xorshift64
//                    stream, seed 0x5EED5EED5EED5EED): literals for Huffman coding, many sequences
//                    for FSE tables, and the same template at the same distance each line, which
//                    is what repeat offsets are for
//   structured_long  700 lines of the same kind from another seed (73,626 bytes), for many blocks
//                    that reuse the trees and tables of the ones before
//   unit             a 16-byte unit and 64 random bytes, forty times over: every sequence is alike,
//                    which is when an encoder writes the sequence tables as RLE
//   far_repeats      421,440 bytes made to reach what the small inputs cannot (see the comment on
//                    farRepeatsBytes): a 112 KiB run of flat noise over 32 symbols, which is a
//                    Huffman literals section of 18-bit sizes (a 5-byte header) in a compressed
//                    block of 72 KB, with a Huffman tree written as direct 4-bit weights; then
//                    copies of it a block and more away, for offsets past 128 KiB
//   mixed3           rle, random and structured end to end, for the three-frame file
//
// The raw inputs are written to the scratch folder and not committed. What is committed, in this
// folder: for each input, `<input>.<variant>.zst` (the encodings), and `<input>.expected.zst`, the
// input itself compressed (`--ultra -22 --long=27 --check`) so it fits in the repository; the
// script checks that `zstd -d` of every encoding gives the input, byte for byte.
//
//   node make_corpus.mjs <scratch dir>
//
//   <scratch dir>  an existing folder: the raw inputs go to <scratch>/in, and the markdown tables
//                  of the README and the Rust file list to <scratch>/tables.md and
//                  <scratch>/files.txt
//
// It is run by hand, once, and is not run in CI. Needs `zstd` (v1.5.7 made the committed files) on
// the PATH and node. It exits non-zero if any decode disagrees with its input. Another zstd version
// may write other bytes for the same input, which is why the README lists the sha256 of what `zstd
// -d` printed, not of the encodings.

import { spawnSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const HERE = import.meta.dirname;
const KTX2_DIR = path.dirname(HERE);
const scratch = process.argv[2];
if (!scratch || !fs.statSync(scratch, { throwIfNoEntry: false })?.isDirectory()) {
  console.error('usage: node make_corpus.mjs <existing scratch dir>');
  process.exit(2);
}
const IN = path.join(scratch, 'in');
fs.mkdirSync(IN, { recursive: true });

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');
const fail = (why) => {
  console.error(`make_corpus: ${why}`);
  process.exit(1);
};

// ── the reference tool ──

const zstd = (args, { stdin, cwd } = {}) => {
  const r = spawnSync('zstd', args, {
    cwd,
    maxBuffer: 1 << 28,
    stdio: [stdin ? fs.openSync(stdin, 'r') : 'ignore', 'pipe', 'pipe'],
  });
  if (r.status !== 0) fail(`zstd ${args.join(' ')} exited ${r.status}: ${r.stderr}`);
  return r;
};
const version = zstd(['--version']).stdout.toString().trim();
if (!version.includes('v1.5.7')) console.error(`make_corpus: warning: the committed files came from v1.5.7, this is ${version}`);

// ── the inputs ──

// xorshift64 (Marsaglia, 2003), shifts 13, 7, 17, as the Rust sweeps use it.
const MASK = (1n << 64n) - 1n;
const xorshift64 = (seed) => {
  let s = seed;
  return () => {
    s ^= (s << 13n) & MASK;
    s ^= s >> 7n;
    s ^= (s << 17n) & MASK;
    return s;
  };
};

const ktx2Level0 = (file) => {
  const k = fs.readFileSync(path.join(KTX2_DIR, file));
  const id = Buffer.from([0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a]);
  if (!k.subarray(0, 12).equals(id)) fail(`${file} is not a KTX2 file`);
  const scheme = k.readUInt32LE(44);
  // The Level Index starts at byte 80: byteOffset, byteLength, uncompressedByteLength, each a u64.
  const [offset, length, uncompressed] = [80, 88, 96].map((at) => Number(k.readBigUInt64LE(at)));
  const level = k.subarray(offset, offset + length);
  if (scheme === 0) {
    if (length !== uncompressed) fail(`${file}: level 0 is ${length} bytes but says ${uncompressed}`);
    return Buffer.from(level);
  }
  if (scheme !== 2) fail(`${file}: supercompression scheme ${scheme}`);
  const frame = path.join(scratch, 'level0.zst');
  fs.writeFileSync(frame, level);
  const out = zstd(['-d', '-c', frame]).stdout;
  if (out.length !== uncompressed) fail(`${file}: level 0 inflates to ${out.length}, not ${uncompressed}`);
  return out;
};

const randomBytes = () => {
  const next = xorshift64(0xc0ffee1234567890n);
  const out = Buffer.alloc(4096);
  for (let at = 0; at < out.length; at += 8) out.writeBigUInt64LE(next(), at);
  return out;
};

// A 16-byte unit and 64 random bytes, forty times over. Every match is the unit (16 bytes, the
// bytes around each copy differ, so no match runs on or back past it) and every literal run is a
// gap of 64 bytes, except the first, which is the first unit and the first gap, 80: the literal
// length codes of 64 and 80 are the same one, and so are all the match length codes. That is when
// the encoder writes the sequence tables as RLE, one byte each.
const unitBytes = () => {
  const next = xorshift64(0x0123456789abcdefn);
  const unit = Buffer.alloc(16);
  unit.writeBigUInt64LE(next(), 0);
  unit.writeBigUInt64LE(next(), 8);
  const parts = [];
  for (let i = 0; i < 40; i++) {
    const gap = Buffer.alloc(64);
    for (let at = 0; at < gap.length; at += 8) gap.writeBigUInt64LE(next(), at);
    gap[0] = i;
    gap[63] = 255 - i;
    parts.push(unit, gap);
  }
  return Buffer.concat(parts);
};

const structuredBytes = (seed, lines) => {
  const next = xorshift64(seed);
  const pick = (list) => list[Number(next() % BigInt(list.length))];
  const number = (below, width) => String(next() % BigInt(below)).padStart(width, '0');
  const MODELS = ['meshy-6', 'tripo-v3', 'rodin-gen-2', 'hunyuan3d-2.1', 'sf3d', 'trellis-2'];
  const FORMATS = ['glb', 'obj', 'stl', 'ply', 'fbx', 'usdz'];
  const STATUS = ['queued', 'running', 'done', 'done', 'done', 'failed'];
  let text = '';
  for (let line = 0; line < lines; line++) {
    text +=
      `${1000 + line} 2026-11-08T${number(24, 2)}:${number(60, 2)}:${number(60, 2)}Z ` +
      `job=${number(1000000, 6)} model=${pick(MODELS).padEnd(13)} format=${pick(FORMATS)} ` +
      `status=${pick(STATUS).padEnd(7)} bytes=${number(5000000, 7)} ms=${number(9000, 4)}\n`;
  }
  return Buffer.from(text);
};

// far_repeats. What a block of 128 KiB or less cannot be, and the small inputs above never are:
//
//   block 0   114,688 bytes of noise, each byte one of 32 values (0 to 31) drawn alike, then the
//             first 16,384 of them again (a match of offset 114,688), which fills the block.
//             Noise over 32 values costs 5 bits a byte, so the noise is a compressed block of 72 KB
//             (past 64 KiB). Its Huffman literals are 114,688 bytes or about that (a
//             section of 16 KiB or more has a header of 5 bytes, the size format 3, and 18-bit
//             sizes). The weights of 32 values drawn alike are all one: the encoder writes them
//             as direct 4-bit weights, as it does for any alphabet that is a power of two.
//   block 1   the 3,840 bytes of uastc_all_modes (UASTC blocks of every mode, kept as literals),
//             then block 0 over again, up to 128 KiB: one match whose offset is 134,912.
//   block 2   about a hundred runs of 16 to 63 fresh noise bytes, each followed by 24 to 2,423
//             bytes copied from somewhere in block 0: offsets from 135,000 to nearly 393,000, so the
//             offsets are of codes 17 and 18, and the repeat offsets seldom repeat.
//   block 3   UASTC bytes, 8 KiB of block 0, 8 KiB of block 2, 8,000 bytes of block 0: the tail.
//
// The window of a file made from this one is its length, 421,440 bytes (the tool clamps it), so
// the offsets are all in reach.
const farRepeatsBytes = (uastc) => {
  const BLOCK = 131072;
  const next = xorshift64(0xfa12c0de5eed0001n);
  const below = (n) => Number((next() >> 16n) % BigInt(n));
  const noise = (len) => {
    const out = Buffer.alloc(len);
    for (let i = 0; i < len; i++) out[i] = Number((next() >> 40n) % 32n);
    return out;
  };
  const fresh = noise(114688);
  const block0 = Buffer.concat([fresh, fresh.subarray(0, BLOCK - fresh.length)]);
  const block1 = Buffer.concat([uastc, block0.subarray(0, BLOCK - uastc.length)]);
  const runs = [];
  for (let have = 0; have < BLOCK; ) {
    const literals = noise(16 + below(48));
    const from = below(BLOCK - 4096);
    const copy = block0.subarray(from, from + 24 + below(2400));
    runs.push(literals, copy);
    have += literals.length + copy.length;
  }
  const block2 = Buffer.concat(runs).subarray(0, BLOCK);
  const block3 = Buffer.concat([uastc, block0.subarray(0, 8192), block2.subarray(0, 8192), block0.subarray(70000, 78000)]);
  return Buffer.concat([block0, block1, block2, block3]);
};

const inputs = {
  uastc_normal: { bytes: ktx2Level0('FlightHelmet_Materials_LensesMat_Normal.ktx2'), origin: 'level 0 of `FlightHelmet_Materials_LensesMat_Normal.ktx2` (UASTC + Zstandard, CC0 1.0), inflated by `zstd -d`' },
  uastc_all_modes: { bytes: ktx2Level0('uastc_all_modes.ktx2'), origin: 'level 0 of `uastc_all_modes.ktx2` (UASTC, scheme 0): 240 blocks, every mode; ours' },
  rle: { bytes: Buffer.alloc(4096, 0xa5), origin: '4,096 x `0xA5`; ours' },
  random: { bytes: randomBytes(), origin: 'xorshift64 seed `0xC0FFEE1234567890`, 512 outputs as u64 little-endian; ours' },
  structured: { bytes: structuredBytes(0x5eed5eed5eed5eedn, 64), origin: '64 fixed-width job-log lines, xorshift64 seed `0x5EED5EED5EED5EED`; ours' },
  structured_long: { bytes: structuredBytes(0x5eed00005eed0001n, 700), origin: '700 of the same lines, xorshift64 seed `0x5EED00005EED0001`; ours' },
  unit: { bytes: unitBytes(), origin: 'a 16-byte unit and 64 more bytes, 40 times (3,200 bytes), xorshift64 seed `0x0123456789ABCDEF`; ours' },
  far_repeats: { bytes: null, origin: 'noise over 32 values, then copies of it past 128 KiB away, with the UASTC blocks of `uastc_all_modes` (421,440 bytes), xorshift64 seed `0xFA12C0DE5EED0001`; ours' },
};
inputs.far_repeats.bytes = farRepeatsBytes(inputs.uastc_all_modes.bytes);
inputs.mixed3 = {
  bytes: Buffer.concat([inputs.rle.bytes, inputs.random.bytes, inputs.structured.bytes]),
  origin: '`rle`, `random` and `structured` end to end; ours',
};
for (const [name, input] of Object.entries(inputs)) {
  input.sha = sha256(input.bytes);
  input.path = path.join(IN, `${name}.bin`);
  fs.writeFileSync(input.path, input.bytes);
}

// ── the encodings ──

const LEVELS = [
  ['l1', ['-1']],
  ['l3', ['-3']],
  ['l9', ['-9']],
  ['l19', ['-19']],
  ['l22', ['--ultra', '-22']],
];
const BASIC = ['uastc_normal', 'uastc_all_modes', 'rle', 'random', 'structured'];

// Each is { file, input, args, stdin } with `stdin` true when the input is fed as a redirect, so
// the frame is written without knowing the content size: no Frame_Content_Size, the window the
// level names (`zstd -3 -c < f` gives 2 MiB, -19 gives 8 MiB, --ultra -22 --long=27 gives 128 MiB).
const specs = [];
for (const input of BASIC) {
  for (const [level, flags] of LEVELS) {
    for (const check of ['check', 'nocheck']) {
      specs.push({ file: `${input}.${level}.${check}.zst`, input, args: [...flags, check === 'check' ? '--check' : '--no-check'] });
    }
  }
  specs.push({ file: `${input}.l19.long27.zst`, input, args: ['-19', '--long=27', '--check'] });
  // A window of 1 KiB, the least there is, caps a block at 1 KiB: the input is many blocks (an
  // encoder never writes an RLE block first in a frame, nor a treeless literals section, a repeat
  // table or a repeat offset without an earlier block), and it is longer than its window, so the
  // decoder has to discard what it has output.
  specs.push({ file: `${input}.w10.l3.check.zst`, input, args: ['-3', '--zstd=wlog=10', '--check'] });
  specs.push({ file: `${input}.w10.l19.nocheck.zst`, input, args: ['-19', '--zstd=wlog=10', '--no-check'] });
  specs.push({ file: `${input}.stream.l3.zst`, input, args: ['-3'], stdin: true });
  specs.push({ file: `${input}.stream.l19.zst`, input, args: ['-19'], stdin: true });
}
for (const input of ['uastc_normal', 'structured']) {
  specs.push({ file: `${input}.stream.l22.long27.zst`, input, args: ['--ultra', '-22', '--long=27'], stdin: true });
}
// `--fast=1000` makes the encoder probe every 1,000th byte, so a long run of one byte goes out as
// literals; the tool leaves literals uncompressed at that level, and `--compress-literals` makes it
// compress them: a run of one byte is a literals section of the RLE kind, which no other setting
// reached. (On the 1 MiB level it only makes a frame of 317 KB, so it is the 4 KiB buffer alone.)
specs.push({ file: 'rle.fast1000.zst', input: 'rle', args: ['--fast=1000', '--compress-literals', '--check'] });
// The sequences of `unit` are all alike: the encoder writes their tables as RLE.
for (const [name, args] of [
  ['l3.check', ['-3', '--check']],
  ['l19.nocheck', ['-19', '--no-check']],
  ['w10.l9.check', ['-9', '--zstd=wlog=10', '--check']],
]) {
  specs.push({ file: `unit.${name}.zst`, input: 'unit', args });
}
// 73 KB of text-like data in blocks of 1 KiB, 4 KiB and 32 KiB windows, and in one block, so that
// blocks after the first can reuse the Huffman tree, the FSE tables and the offsets of the ones
// before.
for (const [name, args] of [
  ['l3.check', ['-3', '--check']],
  ['w10.l19.nocheck', ['-19', '--zstd=wlog=10', '--no-check']],
  ['w12.l9.check', ['-9', '--zstd=wlog=12', '--check']],
  ['w15.l22.check', ['--ultra', '-22', '--zstd=wlog=15', '--check']],
]) {
  specs.push({ file: `structured_long.${name}.zst`, input: 'structured_long', args });
}

// 421 KB: a compressed block of 72 KB, Huffman literals with sizes of 18 bits and a tree of direct
// weights, and offsets of up to 380,000. `-19 --long=27` is not an encoding of its own here: it writes
// the very bytes of the expected output (`--ultra -22 --long=27`), which the check after the
// expected outputs below confirms, so the 79 KB file would only be a copy of that one.
for (const [name, args] of [
  ['l3.check', ['-3', '--check']],
  ['l19.check', ['-19', '--check']],
]) {
  specs.push({ file: `far_repeats.${name}.zst`, input: 'far_repeats', args });
}

const rows = []; // what the README's table lists, in order
const info = (file) => {
  const out = zstd(['-lv', file]).stdout.toString();
  const grab = (re) => Number(re.exec(out)?.[1] ?? 0);
  return {
    frames: grab(/# Zstandard Frames: (\d+)/),
    skippable: grab(/# Skippable Frames: (\d+)/),
    window: grab(/Window Size: .*\((\d+) B\)/),
  };
};

// `zstd -d` of `file` must give `input`'s bytes; the row records the sha256 of what it gave.
const record = (file, input, command, extra = '') => {
  const full = path.join(HERE, file);
  zstd(['-t', '-q', full]);
  const decoded = zstd(['-d', '-c', '-q', full]).stdout;
  if (!decoded.equals(input.bytes)) fail(`zstd -d ${file} is not ${input.name}`);
  const { frames, skippable, window } = info(full);
  rows.push({ file, command, frames, skippable, window, size: fs.statSync(full).size, sha: sha256(decoded), extra });
};

for (const [name, input] of Object.entries(inputs)) input.name = name;

for (const spec of specs) {
  const input = inputs[spec.input];
  const out = path.join(HERE, spec.file);
  const args = ['-q', '-f', ...spec.args];
  if (spec.stdin) {
    zstd([...args, '-c', '-o', out], { stdin: input.path });
  } else {
    zstd([...args, input.path, '-o', out]);
  }
  const bin = `${spec.input}.bin`;
  const command = spec.stdin ? `zstd ${args.join(' ')} -c < ${bin} > ${spec.file}` : `zstd ${args.join(' ')} ${bin} -o ${spec.file}`;
  record(spec.file, input, command);
}

// Three frames end to end: a different input, level and check flag in each.
{
  const parts = [
    ['rle', ['-3', '--no-check']],
    ['random', ['-9', '--check']],
    ['structured', ['-19', '--check']],
  ];
  const frames = parts.map(([name, flags], i) => {
    const part = path.join(scratch, `frame${i}.zst`);
    zstd(['-q', '-f', ...flags, inputs[name].path, '-o', part]);
    return { part, command: `zstd -q -f ${flags.join(' ')} ${name}.bin -o frame${i}.zst` };
  });
  fs.writeFileSync(path.join(HERE, 'mixed3.frames3.zst'), Buffer.concat(frames.map((f) => fs.readFileSync(f.part))));
  record('mixed3.frames3.zst', inputs.mixed3, `${frames.map((f) => f.command).join('; ')}; cat frame0.zst frame1.zst frame2.zst > mixed3.frames3.zst`);
}

// A skippable frame first (RFC 8878 section 3.1.2): the zstd tool has no option to write one, so
// the script does, from the specification: magic 0x184D2A50, a u32 size, that many bytes of user data.
{
  const data = Buffer.from('litegen ktx2 zstd corpus: user data of a skippable frame');
  const header = Buffer.alloc(8);
  header.writeUInt32LE(0x184d2a50, 0);
  header.writeUInt32LE(data.length, 4);
  const skippable = Buffer.concat([header, data]);
  // The same bytes as a shell command (octal escapes: every printf takes them), run to check it.
  const octal = [...header].map((b) => `\\${b.toString(8).padStart(3, '0')}`).join('');
  const printf = `printf '${octal}${data.toString()}' > skippable.bin`;
  const made = spawnSync('sh', ['-c', printf], { cwd: scratch });
  if (made.status !== 0 || !fs.readFileSync(path.join(scratch, 'skippable.bin')).equals(skippable)) fail(`${printf} does not write the skippable frame`);
  const frame = fs.readFileSync(path.join(HERE, 'structured.l19.check.zst'));
  fs.writeFileSync(path.join(HERE, 'structured.skippable-first.zst'), Buffer.concat([skippable, frame]));
  record('structured.skippable-first.zst', inputs.structured, `${printf}; cat skippable.bin structured.l19.check.zst > structured.skippable-first.zst`);
}

// The expected outputs: each input, compressed to fit, as one frame.
const expected = [];
for (const [name, input] of Object.entries(inputs)) {
  const file = `${name}.expected.zst`;
  const args = ['-q', '-f', '--ultra', '-22', '--long=27', '--check'];
  zstd([...args, input.path, '-o', path.join(HERE, file)]);
  expected.push(file);
  record(file, input, `zstd ${args.join(' ')} ${name}.bin -o ${file}`);
}

// `-19 --long=27` of far_repeats must still write the expected output's bytes (see the specs).
{
  const same = path.join(scratch, 'far_repeats.l19.long27.zst');
  zstd(['-q', '-f', '-19', '--long=27', '--check', inputs.far_repeats.path, '-o', same]);
  const identical = fs.readFileSync(same).equals(fs.readFileSync(path.join(HERE, 'far_repeats.expected.zst')));
  console.log(`far_repeats: zstd -19 --long=27 ${identical ? 'writes the same bytes as' : 'DIFFERS from'} the expected output`);
  if (!identical) console.error('make_corpus: warning: add far_repeats.l19.long27.zst to the specs, it is not a copy of the expected output');
}

// ── what the README and the tests list ──

const n = (v) => v.toLocaleString('en-US');
const tables = [];
tables.push('| file | origin | bytes | sha256 |', '|---|---|---:|---|');
for (const [name, input] of Object.entries(inputs)) tables.push(`| \`${name}.bin\` | ${input.origin} | ${n(input.bytes.length)} | \`${input.sha}\` |`);
tables.push('', '| file | command | frames | skippable | window | bytes | sha256 of `zstd -d` |', '|---|---|---:|---:|---:|---:|---|');
for (const r of rows) tables.push(`| \`${r.file}\` | \`${r.command}\` | ${r.frames} | ${r.skippable} | ${n(r.window)} | ${n(r.size)} | \`${r.sha}\` |`);
fs.writeFileSync(path.join(scratch, 'tables.md'), tables.join('\n') + '\n');
fs.writeFileSync(path.join(scratch, 'files.txt'), rows.map((r) => `"${r.file}",`).join('\n') + '\n');

const total = rows.reduce((sum, r) => sum + r.size, 0);
console.log(`${version}`);
console.log(`${rows.length} files (${expected.length} expected outputs), ${n(total)} bytes together`);
