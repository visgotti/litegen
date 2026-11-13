// The pixels of an image, for the one comparison a byte hash cannot make: a
// KTX2 image (KHR_texture_basisu) in an original against the PNG litegen writes
// of it (`normalize::transcode_textures`, mesh-gaps item 3). The bytes differ by
// design, the pixels must not.
//
//   const digest = await loadPixelDigest();   // once; reads three's transcoder
//   digest(bytes)   // 'rgba8 <w>x<h> <sha-256 of the RGBA8 pixels>', or null
//
// A KTX2 image is transcoded to RGBA32 by Binomial's own transcoder, the wasm
// bundled with three r183 (scripts/mesh-verify/node_modules/three/examples/jsm/
// libs/basis/), level 0, layer 0, face 0, exactly as KTX2Loader does for its
// RGBA32 fallback: a reader that shares no code with litegen. A PNG (8 bits, not
// interlaced, grey, grey with alpha, RGB, RGBA or palette) is inflated and
// unfiltered here, and expanded to RGBA8 (an opaque PNG gets alpha 255, which is
// what the transcoder gives an image with no alpha). Anything else gives null,
// and the caller then compares nothing.
//
// The files are untrusted downloads: they are read as data only.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const RGBA32 = 13; // KTX2Loader.TranscoderFormat.RGBA32
const KTX2_ID = [0xab, 0x4b, 0x54, 0x58, 0x20, 0x32, 0x30, 0xbb, 0x0d, 0x0a, 0x1a, 0x0a];
const PNG_ID = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

export const isKtx2 = (bytes) => bytes.length >= 12 && KTX2_ID.every((b, i) => bytes[i] === b);
export const isPng = (bytes) => bytes.length >= 8 && PNG_ID.every((b, i) => bytes[i] === b);

// basis_transcoder.js is CommonJS inside a package whose package.json says
// "type": "module", so it is evaluated as three's KTX2Loader does (the source as
// text, the wasm handed over as `wasmBinary`), with `new Function`.
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
  const basis = await new Promise((resolve, reject) => {
    const m = { wasmBinary, onRuntimeInitialized: () => resolve(m) };
    try {
      mod.exports(m);
    } catch (e) {
      reject(e);
    }
  });
  basis.initializeBasis();
  if (basis.KTX2File === undefined) throw new Error('this transcoder has no KTX2File');
  return basis;
}

function ktx2Pixels(basis, bytes) {
  const ktx = new basis.KTX2File(new Uint8Array(bytes));
  try {
    if (!ktx.isValid() || !ktx.startTranscoding()) return null;
    const [width, height] = [ktx.getWidth(), ktx.getHeight()];
    const size = ktx.getImageTranscodedSizeInBytes(0, 0, 0, RGBA32);
    if (size !== width * height * 4) return null;
    const rgba = new Uint8Array(size);
    if (!ktx.transcodeImage(rgba, 0, 0, 0, RGBA32, 0, -1, -1)) return null;
    return { width, height, rgba };
  } finally {
    ktx.close();
    ktx.delete();
  }
}

const paeth = (a, b, c) => {
  const p = a + b - c;
  const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
};

function pngPixels(bytes) {
  const buf = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let head = null;
  let palette = null;
  let trns = null;
  const idat = [];
  for (let at = 8; at + 12 <= buf.length; ) {
    const len = buf.readUInt32BE(at);
    const type = buf.toString('latin1', at + 4, at + 8);
    const body = buf.subarray(at + 8, at + 8 + len);
    if (type === 'IHDR') head = { width: body.readUInt32BE(0), height: body.readUInt32BE(4), depth: body[8], color: body[9], interlace: body[12] };
    else if (type === 'PLTE') palette = body;
    else if (type === 'tRNS') trns = body;
    else if (type === 'IDAT') idat.push(body);
    else if (type === 'IEND') break;
    at += 12 + len;
  }
  if (!head || head.depth !== 8 || head.interlace !== 0 || !idat.length) return null;
  const { width, height, color } = head;
  const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[color];
  if (!channels || (color === 3 && !palette) || width * height > 1 << 26) return null;
  const stride = width * channels;
  // The header says how many bytes the image inflates to; an IDAT that holds
  // more is refused as it inflates (a decompression bomb costs nothing here).
  const raw = zlib.inflateSync(Buffer.concat(idat), { maxOutputLength: (stride + 1) * height });
  if (raw.length !== (stride + 1) * height) return null;
  const px = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x++) {
      const v = raw[y * (stride + 1) + 1 + x];
      const left = x >= channels ? px[y * stride + x - channels] : 0;
      const up = y ? px[(y - 1) * stride + x] : 0;
      const upLeft = y && x >= channels ? px[(y - 1) * stride + x - channels] : 0;
      let add;
      if (filter === 0) add = 0;
      else if (filter === 1) add = left;
      else if (filter === 2) add = up;
      else if (filter === 3) add = (left + up) >> 1;
      else if (filter === 4) add = paeth(left, up, upLeft);
      else return null;
      px[y * stride + x] = (v + add) & 255;
    }
  }
  const rgba = new Uint8Array(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const o = i * 4;
    if (color === 0) rgba.set([px[i], px[i], px[i], 255], o);
    else if (color === 2) rgba.set([px[i * 3], px[i * 3 + 1], px[i * 3 + 2], 255], o);
    else if (color === 3) {
      const k = px[i];
      rgba.set([palette[k * 3] ?? 0, palette[k * 3 + 1] ?? 0, palette[k * 3 + 2] ?? 0, trns && k < trns.length ? trns[k] : 255], o);
    } else if (color === 4) rgba.set([px[i * 2], px[i * 2], px[i * 2], px[i * 2 + 1]], o);
    else rgba.set(px.subarray(i * 4, i * 4 + 4), o);
  }
  return { width, height, rgba };
}

const BASIS_DIR = path.join(here, 'node_modules', 'three', 'examples', 'jsm', 'libs', 'basis');

// `{ width, height, rgba }` of a KTX2 or PNG image, or null (not one, or
// not a kind this reads). The tests build their own PNGs and read the pixels.
export async function loadPixelReader(basisDir = BASIS_DIR) {
  const basis = await loadBasis(basisDir);
  return (bytes) => {
    try {
      if (isKtx2(bytes)) return ktx2Pixels(basis, bytes);
      if (isPng(bytes)) return pngPixels(bytes);
    } catch {
      // A broken image reads as nothing.
    }
    return null;
  };
}

export async function loadPixelDigest(basisDir = BASIS_DIR) {
  const read = await loadPixelReader(basisDir);
  return (bytes) => {
    const img = read(bytes);
    return img ? `rgba8 ${img.width}x${img.height} ${crypto.createHash('sha256').update(img.rgba).digest('hex')}` : null;
  };
}
