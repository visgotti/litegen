#!/usr/bin/env node
// make_reference.mjs: writes the expected-output files of litegen's KTX2 corpus.
//
// For each .ktx2 given, this transcodes level 0, layer 0, face 0 to RGBA32 with the Basis
// Universal transcoder that ships with three.js, and writes the raw bytes (width x height x 4,
// rows top to bottom, no header, no padding) as <out dir>/<name>.rgba. Task 2 and 3's test,
// `every_reference_texture_transcodes_bit_exactly`, compares litegen's own decoder with these
// files byte for byte. It is run by hand, once, and is not run in CI.
//
//   node make_reference.mjs <transcoder dir> <out dir> <input.ktx2>...
//
// The raw .rgba files are 12.7 MB, so they are not committed: the repository holds each one
// compressed as <name>.rgba.zst (Ruling K10), by the zstd command line tool (v1.5.7), and the
// tests inflate them with ruzstd and compare byte for byte. After this script, from the folder
// that holds the .ktx2 files, with <out dir> the folder it wrote the .rgba files into:
//
//   for f in <out dir>/*.rgba; do
//     zstd -q -f --ultra -22 --long=27 -o "$(basename "$f").zst" "$f"
//   done
//
// (-22 is zstd's slowest level and --long=27 its widest window; the frame still names a window
// of the file's own size. A different zstd version may write different bytes. What a reference
// is, is its raw sha256, which the README lists and the script prints.) Write the .rgba files
// to a scratch folder, not this one, and do not commit them.
//
//   <transcoder dir>  holds basis_transcoder.js and basis_transcoder.wasm, e.g.
//                     scripts/mesh-verify/node_modules/three/examples/jsm/libs/basis
//                     (three 0.183.2; the files are read, never modified or copied)
//   <out dir>         an existing folder to write the .rgba files into
//   <input.ktx2>      the textures; paths are arguments, nothing is read from the cwd
//
// It prints one JSON line per input (sizes, sha256 of the input and of the reference, and what
// the file says it is), and exits non-zero on any disagreement, so a wrong run cannot pass as a
// quiet one. Each fact the transcoder reports (KTX2File.getWidth, getLevels, isETC1S, ...) is
// checked against the same fact read straight from the file's bytes by the small parser below,
// which follows the KTX File Format Specification v2.0: the Header and Index (sections 3.2 to
// 3.9), the Data Format Descriptor (3.10) and BasisLZ's global data (3.12 and Appendix C).
//
// What it relies on of three.js (r183, examples/jsm/loaders/KTX2Loader.js, function `transcode`),
// which makes the same calls to get its own RGBA32 pixels:
//   new BasisModule.KTX2File(bytes); isValid(); isUASTC() / isETC1S(); getWidth(); getHeight();
//   getLayers(); getLevels(); getFaces(); getHasAlpha(); startTranscoding();
//   getImageTranscodedSizeInBytes(level, layer, 0, format);
//   transcodeImage(dst, level, layer, face, format, 0, -1, -1); close(); delete().
// and of three's KTX2Loader.TranscoderFormat for the target: RGBA32 is 13.
// The transcoder module exposes no version string, so the output records what identifies it:
// the sha256 and size of both files, and three's package.json version.

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const RGBA32 = 13; // KTX2Loader.TranscoderFormat.RGBA32

const sha256 = (bytes) => crypto.createHash('sha256').update(bytes).digest('hex');

// basis_transcoder.js is CommonJS, but it sits in three's package, whose package.json says
// "type": "module", so require() or import of that path would treat it as ESM. three's own
// KTX2Loader reads the source as text and evaluates it in a worker, giving the module the wasm
// bytes as `wasmBinary` (KTX2Loader.js, `init`). This does the same with `new Function`.
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

// What the file's own bytes say. All fields little-endian.
function readHeader(bytes, name) {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const id = [0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a];
  if (bytes.length < 80 || id.some((b, i) => bytes[i] !== b)) throw new Error(`${name}: not a KTX2 file`);
  const h = {
    vkFormat: dv.getUint32(12, true),
    width: dv.getUint32(20, true),
    height: dv.getUint32(24, true),
    depth: dv.getUint32(28, true),
    layers: dv.getUint32(32, true),
    faces: dv.getUint32(36, true),
    levelCount: dv.getUint32(40, true),
    scheme: dv.getUint32(44, true),
    dfdOffset: dv.getUint32(48, true),
    dfdLength: dv.getUint32(52, true),
    sgdOffset: Number(dv.getBigUint64(64, true)),
    sgdLength: Number(dv.getBigUint64(72, true)),
  };
  // DFD: dfdTotalSize, then the basic block's words; colorModel is byte 0 of word 3 (byte 12).
  h.colorModel = bytes[h.dfdOffset + 12];
  h.transfer = bytes[h.dfdOffset + 14];
  // BasisLZ global data: a 20-byte header, then one 20-byte ImageDesc per image (Appendix C):
  // imageFlags, rgbSliceByteOffset, rgbSliceByteLength, alphaSliceByteOffset, alphaSliceByteLength.
  if (h.scheme === 1) {
    const at = h.sgdOffset + 20;
    h.alphaSliceByteLength = dv.getUint32(at + 16, true);
    h.rgbSliceByteLength = dv.getUint32(at + 8, true);
  }
  return h;
}

function main() {
  const [dir, outDir, ...inputs] = process.argv.slice(2);
  if (!dir || !outDir || inputs.length === 0) {
    console.error('usage: node make_reference.mjs <transcoder dir> <out dir> <input.ktx2>...');
    process.exit(2);
  }
  return run(dir, outDir, inputs);
}

async function run(dir, outDir, inputs) {
  if (!fs.statSync(outDir).isDirectory()) throw new Error(`${outDir} is not a folder`);
  const basis = await loadBasis(dir);
  let three = null;
  try {
    three = JSON.parse(fs.readFileSync(path.join(dir, '../../../../package.json'), 'utf8')).version;
  } catch {
    // not inside three's tree: leave it out
  }
  const transcoderFiles = Object.fromEntries(
    ['basis_transcoder.js', 'basis_transcoder.wasm'].map((f) => {
      const b = fs.readFileSync(path.join(dir, f));
      return [f, { bytes: b.length, sha256: sha256(b) }];
    }),
  );
  console.log(JSON.stringify({ transcoder: transcoderFiles, three, node: process.version, target: 'RGBA32 (13)' }));

  let failures = 0;
  for (const file of inputs) {
    const name = path.basename(file);
    const stem = name.replace(/\.ktx2$/, '');
    try {
      const bytes = new Uint8Array(fs.readFileSync(file));
      const h = readHeader(bytes, name);
      const ktx = new basis.KTX2File(bytes);
      try {
        if (!ktx.isValid()) throw new Error('the transcoder says the file is invalid');
        const facts = {
          width: ktx.getWidth(),
          height: ktx.getHeight(),
          levels: ktx.getLevels(),
          layers: ktx.getLayers(),
          faces: ktx.getFaces(),
          etc1s: ktx.isETC1S(),
          uastc: ktx.isUASTC(),
          hasAlpha: ktx.getHasAlpha(),
          colorModel: ktx.getDFDColorModel(),
          transfer: ktx.getDFDTransferFunc(),
        };
        const agree =
          facts.width === h.width &&
          facts.height === h.height &&
          facts.levels === h.levelCount &&
          facts.faces === h.faces &&
          facts.layers === h.layers &&
          facts.colorModel === h.colorModel &&
          facts.transfer === h.transfer &&
          facts.etc1s === (h.colorModel === 163) &&
          facts.uastc === (h.colorModel === 166) &&
          h.vkFormat === 0 &&
          h.depth === 0 &&
          h.faces === 1 &&
          (h.layers === 0 || h.layers === 1);
        if (!agree) throw new Error(`the transcoder and the file's bytes disagree: ${JSON.stringify({ facts, h })}`);
        // ETC1S carries alpha as a second slice; the transcoder's flag must say the same.
        const alphaSlice = h.scheme === 1 ? h.alphaSliceByteLength > 0 : null;
        if (alphaSlice !== null && alphaSlice !== facts.hasAlpha) {
          throw new Error(`alpha slice ${alphaSlice} but the transcoder's getHasAlpha is ${facts.hasAlpha}`);
        }
        if (!ktx.startTranscoding()) throw new Error('startTranscoding failed');
        const size = ktx.getImageTranscodedSizeInBytes(0, 0, 0, RGBA32);
        if (size !== facts.width * facts.height * 4) throw new Error(`RGBA32 size ${size} is not width x height x 4`);
        const transcode = () => {
          const dst = new Uint8Array(size);
          if (!ktx.transcodeImage(dst, 0, 0, 0, RGBA32, 0, -1, -1)) throw new Error('transcodeImage failed');
          return dst;
        };
        const rgba = transcode();
        if (sha256(rgba) !== sha256(transcode())) throw new Error('two transcodes of level 0 differ');
        let translucent = 0;
        for (let i = 3; i < rgba.length; i += 4) if (rgba[i] !== 255) translucent++;
        fs.writeFileSync(path.join(outDir, `${stem}.rgba`), rgba);
        console.log(
          JSON.stringify({
            file: name,
            bytes: bytes.length,
            sha256: sha256(bytes),
            format: facts.etc1s ? 'ETC1S' : 'UASTC',
            scheme: ['none', 'BasisLZ', 'Zstandard', 'ZLIB'][h.scheme] ?? h.scheme,
            width: facts.width,
            height: facts.height,
            levels: facts.levels,
            dfdTransfer: facts.transfer === 2 ? 'sRGB' : facts.transfer === 1 ? 'linear' : facts.transfer,
            alphaSlice,
            translucentPixels: translucent,
            reference: `${stem}.rgba`,
            referenceBytes: rgba.length,
            referenceSha256: sha256(rgba),
          }),
        );
      } finally {
        ktx.close();
        ktx.delete();
      }
    } catch (e) {
      failures++;
      console.error(`${name}: ${e.message}`);
    }
  }
  if (failures) process.exit(1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
