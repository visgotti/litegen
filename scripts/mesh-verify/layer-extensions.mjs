#!/usr/bin/env node
// The layer-extension check (Ruling A20): a SEMANTIC comparison of a glTF
// source's material layers with litegen's glTF output, material by material.
// verify.mjs runs it on every glTF output of a glTF original (an `--import`
// run), and the README's "The layer-extension check" says how and why; it
// also runs alone:
//
//   node layer-extensions.mjs <source.glb|.gltf> <output.glb|.gltf> [<output> ...] [--json <file>]
//
// Ruling IDs (A20, A20a) are mesh-gaps item 4's rulings, listed with their
// costs in the status block of
// docs/superpowers/plans/2026-11-04-mesh-material-layers.md.
//
// Per material pair, it requires:
//  - the same set of the nine layer extensions (KHR_materials_clearcoat,
//    transmission, volume, ior, specular, sheen, iridescence, anisotropy,
//    dispersion);
//  - each field equal after filling the extension's schema defaults
//    (DEFAULTS); a field the schema does not name is compared as given;
//  - each texture compared by the sha-256 of the image bytes it resolves to,
//    its texCoord (0 when absent; a KHR_texture_transform texCoord overrides
//    it, as that extension says) and its KHR_texture_transform (identity when
//    absent), plus a normal texture's `scale` (default 1). A texture whose
//    image resolves on either side to nothing is a failure, never equal. One
//    byte difference is by design: a KTX2 image (KHR_texture_basisu) of the
//    source that litegen's production path wrote as PNG (mesh-gaps item 3).
//    It is equal when the two decode to the same RGBA8 (pixel-digest.mjs:
//    three's Binomial transcoder, and a PNG reader of its own), listed as
//    `ktx2Pixels`; without `usePixelDigest` it is the difference it looks like;
//  - no output material that pairs with no source material carries a layer.
// Floats compare as f32 (Ruling A20a): two numbers that round to the same
// f32 are equal, since the scene holds f32; each such pair is listed as
// information (`f32Rounded`), and any other difference fails, however small.
// Excluded, and listed: `extras` and a layer object's own nested
// `extensions` (and a layer texture's extras and extensions other than
// KHR_texture_transform). Listed as information: every other material
// extension the source has and whether litegen kept it, and
// KHR_materials_variants (a non-goal, warned on read): a material only a
// variant mapping uses is listed when litegen drops it, not failed.
//
// The files are untrusted downloads: they are read as data only, and a uri
// that leaves the file's own folder is refused.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { depthFirstRanks } from './page/node-rank.js';
import { isKtx2 } from './pixel-digest.mjs';

export const LAYERS = ['clearcoat', 'transmission', 'volume', 'ior', 'specular', 'sheen', 'iridescence', 'anisotropy', 'dispersion'];

// The schema defaults, from KhronosGroup/glTF at 9d23a5a5b1d10f22d1554e847f2a1a3f24222392,
// extensions/2.0/Khronos/KHR_materials_<layer>/schema/material.KHR_materials_<layer>.schema.json
// (pinned by layer-extensions.test.mjs against copies of those files; the
// README says how to fetch them). Textures carry no default:
// absent means no texture. attenuationDistance has no `default` key; its
// description says "When undefined, the value is assumed to be infinite."
export const DEFAULTS = {
  KHR_materials_clearcoat: { clearcoatFactor: 0, clearcoatRoughnessFactor: 0 },
  KHR_materials_transmission: { transmissionFactor: 0 },
  KHR_materials_volume: { thicknessFactor: 0, attenuationDistance: Infinity, attenuationColor: [1, 1, 1] },
  KHR_materials_ior: { ior: 1.5 },
  KHR_materials_specular: { specularFactor: 1, specularColorFactor: [1, 1, 1] },
  KHR_materials_sheen: { sheenColorFactor: [0, 0, 0], sheenRoughnessFactor: 0 },
  KHR_materials_iridescence: { iridescenceFactor: 0, iridescenceIor: 1.3, iridescenceThicknessMinimum: 100, iridescenceThicknessMaximum: 400 },
  KHR_materials_anisotropy: { anisotropyStrength: 0, anisotropyRotation: 0 },
  KHR_materials_dispersion: { dispersion: 0 },
};
// The one normalTextureInfo among the layers' textures (its `scale`, default 1:
// specification/2.0/schema/material.normalTextureInfo.schema.json).
const NORMAL_TEXTURES = new Set(['clearcoatNormalTexture']);

const sha256 = (b) => crypto.createHash('sha256').update(b).digest('hex');

// ─── reading ────────────────────────────────────────────────────────────────

export function readGltf(file) {
  const bytes = fs.readFileSync(file);
  const dir = path.dirname(path.resolve(file));
  let json;
  let bin = null;
  if (bytes.length >= 12 && bytes.readUInt32LE(0) === 0x46546c67) {
    for (let off = 12; off + 8 <= bytes.length; ) {
      const len = bytes.readUInt32LE(off);
      const type = bytes.readUInt32LE(off + 4);
      const body = bytes.subarray(off + 8, off + 8 + len);
      if (type === 0x4e4f534a) json = JSON.parse(body.toString('utf8'));
      else if (type === 0x004e4942 && bin === null) bin = body;
      off += 8 + len;
    }
    if (!json) throw new Error(`${file}: no JSON chunk`);
  } else {
    json = JSON.parse(bytes.toString('utf8'));
  }
  const buffers = new Map();
  const buffer = (i) => {
    if (!buffers.has(i)) {
      const b = json.buffers?.[i];
      if (!b) throw new Error(`${file}: no buffer ${i}`);
      buffers.set(i, b.uri === undefined ? bin : fromUri(b.uri, dir));
    }
    return buffers.get(i);
  };
  const imageBytes = (i) => {
    const im = json.images?.[i];
    if (!im) return null;
    if (im.bufferView !== undefined) {
      const bv = json.bufferViews[im.bufferView];
      const at = bv.byteOffset || 0;
      return buffer(bv.buffer).subarray(at, at + bv.byteLength);
    }
    return im.uri === undefined ? null : fromUri(im.uri, dir);
  };
  return { file, json, imageBytes };
}

function fromUri(uri, dir) {
  const m = /^data:[^,]*?(;base64)?,(.*)$/s.exec(uri);
  if (m) return m[1] ? Buffer.from(m[2], 'base64') : Buffer.from(decodeURIComponent(m[2]), 'latin1');
  const p = path.resolve(dir, decodeURIComponent(uri));
  if (!p.startsWith(dir + path.sep)) throw new Error(`a uri leaves its file's folder: ${uri}`);
  return fs.readFileSync(p);
}

// ─── canonical forms ────────────────────────────────────────────────────────

// The image texture `index` draws: its source index (undefined for none) and
// its bytes (null for none).
function textureSource(doc, index) {
  const t = doc.json.textures?.[index];
  if (!t) return { missing: `unresolved (no texture ${index})` };
  let image = t.source;
  if (image === undefined) {
    // A texture whose image sits in an extension (KHR_texture_basisu,
    // EXT_texture_webp, ...): its first such source.
    for (const v of Object.values(t.extensions || {})) if (v && v.source !== undefined) { image = v.source; break; }
  }
  return { image, bytes: image === undefined ? null : doc.imageBytes(image) };
}

// The sha-256 of the image texture `index` draws, or a text that starts
// with "unresolved" when it draws none: never equal to another texture's.
function textureImage(doc, index) {
  const { missing, image, bytes } = textureSource(doc, index);
  if (missing) return missing;
  return bytes ? sha256(bytes) : `unresolved (texture ${index}, image ${image})`;
}

// The one image difference that is by design (mesh-gaps item 3): litegen's
// production path (`normalize::transcode_textures`) writes a KTX2 image of its
// source as PNG, so the bytes differ and the pixels must not. `usePixelDigest`
// (pixel-digest.mjs, which verify.mjs sets once) decodes both sides with readers
// that share nothing with litegen. Without it a KTX2 source against a PNG output
// is the byte difference it looks like.
let pixelDigest = null;
export function usePixelDigest(fn) {
  pixelDigest = fn;
}
// `a` is the source file's texture and `b` the output's: only a KTX2 source with
// a non-KTX2 output is the by-design difference (the reverse is not).
function ktx2AsPng(a, b) {
  if (!pixelDigest || !a.bytes || !b.bytes || !isKtx2(a.bytes) || isKtx2(b.bytes)) return null;
  const [x, y] = [pixelDigest(a.bytes), pixelDigest(b.bytes)];
  return x !== null && x === y ? x : null;
}

function canonTexture(doc, field, info, excluded, at) {
  const tt = info.extensions?.KHR_texture_transform || {};
  const c = {
    image: textureImage(doc, info.index),
    texCoord: tt.texCoord ?? info.texCoord ?? 0,
    transform: { offset: tt.offset ?? [0, 0], rotation: tt.rotation ?? 0, scale: tt.scale ?? [1, 1] },
    other: {},
  };
  // Not part of the canonical form (not enumerable): ktx2AsPng reads it.
  Object.defineProperty(c, 'bytes', { value: textureSource(doc, info.index).bytes ?? null });
  if (NORMAL_TEXTURES.has(field)) c.scale = info.scale ?? 1;
  for (const [k, v] of Object.entries(info)) {
    if (k === 'extras') excluded.push(`${at}.extras`);
    else if (k === 'extensions') {
      for (const e of Object.keys(v || {})) if (e !== 'KHR_texture_transform') excluded.push(`${at}.extensions.${e}`);
    } else if (!['index', 'texCoord'].includes(k) && !(k === 'scale' && NORMAL_TEXTURES.has(field))) c.other[k] = v;
  }
  return c;
}

const isTextureInfo = (v) => v && typeof v === 'object' && !Array.isArray(v) && Number.isInteger(v.index);

function canonLayer(doc, ext, obj, excluded) {
  const c = { ...DEFAULTS[ext] };
  for (const [k, v] of Object.entries(obj || {})) {
    if (k === 'extras') excluded.push(`${ext}.extras`);
    else if (k === 'extensions') for (const e of Object.keys(v || {})) excluded.push(`${ext}.extensions.${e}`);
    else c[k] = isTextureInfo(v) ? { texture: canonTexture(doc, k, v, excluded, `${ext}.${k}`) } : v;
  }
  return c;
}

// ─── comparing ──────────────────────────────────────────────────────────────

const sortedJson = (v) =>
  JSON.stringify(v, (_, x) => (x && typeof x === 'object' && !Array.isArray(x) ? Object.fromEntries(Object.entries(x).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) : x));

function diffValue(a, b, field, push) {
  if (typeof a === 'number' && typeof b === 'number') {
    if (a !== b) push({ field, src: a, out: b, kind: Math.fround(a) === Math.fround(b) ? 'f32-rounding' : 'value' });
    return;
  }
  if (Array.isArray(a) && Array.isArray(b) && a.length === b.length) {
    a.forEach((x, i) => diffValue(x, b[i], `${field}[${i}]`, push));
    return;
  }
  if (a === undefined && b === undefined) return;
  if (sortedJson(a) !== sortedJson(b)) push({ field, src: a ?? null, out: b ?? null, kind: 'value' });
}

const unresolved = (image) => image.startsWith('unresolved');

function diffTexture(a, b, field, push) {
  if (a && !b) return push({ field, src: `image ${a.image}`, out: null, kind: 'texture-missing' });
  if (!a && b) return push({ field, src: null, out: `image ${b.image}`, kind: 'texture-added' });
  // An image neither side gives compares nothing, so it can't be equal.
  if (unresolved(a.image) || unresolved(b.image)) push({ field: `${field}.image`, src: a.image, out: b.image, kind: 'texture-unresolved' });
  else if (a.image !== b.image) {
    const pixels = ktx2AsPng(a, b);
    if (pixels) push({ field: `${field}.image`, src: a.image, out: b.image, kind: 'ktx2-pixels', pixels });
    else push({ field: `${field}.image`, src: a.image, out: b.image, kind: 'texture-image' });
  }
  diffValue(a.texCoord, b.texCoord, `${field}.texCoord`, push);
  diffValue(a.transform.offset, b.transform.offset, `${field}.transform.offset`, push);
  diffValue(a.transform.rotation, b.transform.rotation, `${field}.transform.rotation`, push);
  diffValue(a.transform.scale, b.transform.scale, `${field}.transform.scale`, push);
  diffValue(a.scale, b.scale, `${field}.scale`, push);
  for (const k of [...new Set([...Object.keys(a.other), ...Object.keys(b.other)])].sort()) diffValue(a.other[k], b.other[k], `${field}.${k}`, push);
}

function layersOf(m) {
  return LAYERS.filter((l) => m?.extensions?.[`KHR_materials_${l}`] !== undefined).sort();
}

// The order litegen's reader keeps a glTF's materials in, as source indices
// (followups 2026-11-08, F). The reader allocates a material when the first
// primitive that uses it is read (glb.rs `material`, L2191-L2266, called at the
// end of `primitive`, L2171-L2175), walking the scene's nodes depth-first and
// reading each node's mesh at its first sight (`nodes`, L2794-L2864 and
// L2868-L2873; a file with no scene takes every mesh in file order, L2800-L2807),
// and the writer writes `scene.materials` in that order (glb.rs L4040-L4121,
// no reordering). So a material no primitive of the scene uses is not kept,
// and one that is comes back at its place in the order of first use, which is
// the file's own order only when the file was already written that way.
export function readerMaterialOrder(json) {
  const meshes = json.meshes || [];
  const order = [];
  const seen = new Set();
  const read = (mi) => {
    for (const p of meshes[mi]?.primitives || []) {
      if (p.material === undefined || seen.has(p.material)) continue;
      seen.add(p.material);
      order.push(p.material);
    }
  };
  if (!(json.scenes || []).length) {
    meshes.forEach((_, mi) => read(mi));
    return order;
  }
  const ranks = depthFirstRanks(json);
  const walk = (json.nodes || []).map((n, i) => [ranks[i], n]).filter(([r]) => r !== undefined).sort((a, b) => a[0] - b[0]);
  const meshSeen = new Set();
  for (const [, n] of walk) {
    if (n.mesh === undefined || meshSeen.has(n.mesh)) continue;
    meshSeen.add(n.mesh);
    read(n.mesh);
  }
  return order;
}

// Which source material goes with which output material: by index when the
// counts match and every source name is kept at its index (an unnamed
// source material matches whatever stands there); else, when a name is shared
// by two materials the reader keeps (so names alone cannot pair them), in the
// order the reader keeps them (readerMaterialOrder), when that makes the
// counts match and every source name is kept at its place, which pairs
// repeated names one to one; else by unique name (a name used by more than
// one material pairs with none).
function pairMaterials(S, O, srcJson) {
  if (S.length === O.length && S.every((m, i) => m.name === undefined || m.name === O[i].name)) {
    return { pairing: 'index', pairs: S.map((_, i) => [i, i]) };
  }
  const order = readerMaterialOrder(srcJson);
  const shared = order.some((si, k) => S[si]?.name !== undefined && order.some((sj, l) => l !== k && S[sj]?.name === S[si].name));
  if (shared && order.length === O.length && order.every((si, k) => S[si] && (S[si].name === undefined || S[si].name === O[k].name))) {
    const at = new Map(order.map((si, k) => [si, k]));
    return { pairing: 'reader order', pairs: S.map((_, si) => [si, at.get(si) ?? null]) };
  }
  const unique = (list) => {
    const seen = new Map();
    list.forEach((m, i) => m.name !== undefined && seen.set(m.name, seen.has(m.name) ? -1 : i));
    return seen;
  };
  const so = unique(O);
  const ss = unique(S);
  return { pairing: 'name', pairs: S.map((m, i) => [i, m.name !== undefined && ss.get(m.name) === i && so.get(m.name) >= 0 ? so.get(m.name) : null]) };
}

function bindings(json) {
  const bound = new Set();
  const variantMapped = new Set();
  let primitives = 0;
  for (const me of json.meshes || []) {
    for (const p of me.primitives || []) {
      if (p.material !== undefined) bound.add(p.material);
      const v = p.extensions?.KHR_materials_variants;
      if (v) {
        primitives++;
        for (const mp of v.mappings || []) variantMapped.add(mp.material);
      }
    }
  }
  return { bound, variantMapped, primitives };
}

export function compareFiles(srcFile, outFile) {
  const src = readGltf(srcFile);
  const out = readGltf(outFile);
  const S = src.json.materials || [];
  const O = out.json.materials || [];
  const label = (m, i) => m.name ?? `#${i}`;
  const { pairing, pairs } = pairMaterials(S, O, src.json);
  const sb = bindings(src.json);
  const ob = bindings(out.json);
  const result = {
    source: srcFile,
    output: outFile,
    pairing,
    verdict: 'pass',
    diffs: [],
    f32Rounded: [],
    ktx2Pixels: [],
    materials: [],
    excluded: [],
    otherExtensions: {},
    variants: { root: !!src.json.extensions?.KHR_materials_variants, primitives: sb.primitives, kept: !!out.json.extensions?.KHR_materials_variants || ob.primitives > 0 },
    variantOnly: [],
    unpaired: { src: [], out: [] },
    extensionsRequired: out.json.extensionsRequired || [],
  };
  const pairedOut = new Set();
  for (const [si, oi] of pairs) {
    const sm = S[si];
    if (oi === null) {
      if (!sb.bound.has(si) && sb.variantMapped.has(si)) result.variantOnly.push(label(sm, si));
      else result.unpaired.src.push(label(sm, si));
      continue;
    }
    pairedOut.add(oi);
    const om = O[oi];
    const name = label(sm, si);
    const entry = { name, src: si, out: oi, layers: { src: layersOf(sm), out: layersOf(om) }, diffs: 0 };
    result.materials.push(entry);
    const excludedHere = [];
    const outExcluded = [];
    const push = (layer) => (d) => {
      if (d.kind === 'f32-rounding') {
        const { kind, ...same } = d;
        result.f32Rounded.push({ material: name, layer, ...same });
        return;
      }
      if (d.kind === 'ktx2-pixels') {
        const { kind, ...same } = d;
        result.ktx2Pixels.push({ material: name, layer, ...same });
        return;
      }
      result.diffs.push({ material: name, layer, ...d });
      entry.diffs++;
    };
    for (const layer of [...new Set([...entry.layers.src, ...entry.layers.out])].sort()) {
      const ext = `KHR_materials_${layer}`;
      const a = sm.extensions?.[ext];
      const b = om.extensions?.[ext];
      if (a === undefined) {
        push(layer)({ field: '(layer)', src: null, out: 'present', kind: 'layer-added' });
        continue;
      }
      if (b === undefined) {
        push(layer)({ field: '(layer)', src: 'present', out: null, kind: 'layer-missing' });
        canonLayer(src, ext, a, excludedHere);
        continue;
      }
      const ca = canonLayer(src, ext, a, excludedHere);
      const cb = canonLayer(out, ext, b, outExcluded);
      for (const k of [...new Set([...Object.keys(ca), ...Object.keys(cb)])].sort()) {
        if (ca[k]?.texture || cb[k]?.texture) diffTexture(ca[k]?.texture, cb[k]?.texture, k, push(layer));
        else diffValue(ca[k], cb[k], k, push(layer));
      }
    }
    for (const p of excludedHere) result.excluded.push({ material: name, path: p });
    for (const p of outExcluded) result.excluded.push({ material: name, path: p, side: 'out' });
    for (const e of Object.keys(sm.extensions || {})) {
      if (LAYERS.includes(e.replace(/^KHR_materials_/, '')) && e.startsWith('KHR_materials_')) continue;
      const o = (result.otherExtensions[e] ??= { src: 0, kept: 0 });
      o.src++;
      if (om.extensions?.[e] !== undefined) o.kept++;
    }
  }
  // Other extensions of source materials that found no partner count as not kept.
  for (const [si, oi] of pairs) {
    if (oi !== null) continue;
    for (const e of Object.keys(S[si].extensions || {})) {
      if (e.startsWith('KHR_materials_') && LAYERS.includes(e.slice('KHR_materials_'.length))) continue;
      (result.otherExtensions[e] ??= { src: 0, kept: 0 }).src++;
    }
  }
  // An unpaired output material is judged by its own index: a label can
  // name another material too (an unnamed one's `#2` and one named "#2").
  const unpairedOut = [];
  O.forEach((m, i) => {
    if (!pairedOut.has(i)) {
      result.unpaired.out.push(label(m, i));
      unpairedOut.push(i);
    }
  });
  const outLayerless = unpairedOut.every((i) => layersOf(O[i]).length === 0);
  if (result.diffs.length || result.unpaired.src.length || !outLayerless) result.verdict = 'fail';
  return result;
}

// ─── report ─────────────────────────────────────────────────────────────────

export function formatResult(r) {
  const kinds = {};
  for (const d of r.diffs) kinds[d.kind] = (kinds[d.kind] || 0) + 1;
  const layered = r.materials.filter((m) => m.layers.src.length || m.layers.out.length);
  const lines = [
    `${r.verdict === 'pass' ? 'PASS' : 'FAIL'} ${r.source} -> ${r.output}`,
    `  pairing by ${r.pairing}: ${r.materials.length} material pair(s), ${layered.length} with a layer; layer objects compared: ${layered.reduce((s, m) => s + m.layers.src.length, 0)}`,
    `  differences: ${r.diffs.length}${r.diffs.length ? ' (' + Object.entries(kinds).map(([k, v]) => `${v} ${k}`).join(', ') + ')' : ''}`,
  ];
  for (const d of r.diffs) lines.push(`    ${d.material} / ${d.layer} / ${d.field}: ${fmt(d.src)} -> ${fmt(d.out)}  [${d.kind}]`);
  lines.push(`  equal as f32 (Ruling A20a), listed: ${r.f32Rounded.length}${r.f32Rounded.length ? ' — e.g. ' + r.f32Rounded.slice(0, 2).map((d) => `${d.material} / ${d.layer} / ${d.field}: ${fmt(d.src)} -> ${fmt(d.out)}`).join('; ') : ''}`);
  lines.push(`  excluded (extras, nested extensions): ${r.excluded.length}${r.excluded.length ? ' — ' + r.excluded.map((e) => `${e.material}: ${e.path}${e.side ? ' (output)' : ''}`).join('; ') : ''}`);
  const other = Object.entries(r.otherExtensions);
  lines.push(`  other material extensions: ${other.length ? other.map(([e, c]) => `${e} on ${c.src} source material(s), kept on ${c.kept}`).join('; ') : 'none'}`);
  lines.push(`  KHR_materials_variants: ${r.variants.root || r.variants.primitives ? `in the source (root ${r.variants.root}, ${r.variants.primitives} primitive(s)), in the output ${r.variants.kept}; variant-only materials dropped: ${r.variantOnly.length ? r.variantOnly.join(', ') : 'none'}` : 'not used'}`);
  if (r.unpaired.src.length || r.unpaired.out.length) lines.push(`  UNPAIRED source: [${r.unpaired.src.join(', ')}], output: [${r.unpaired.out.join(', ')}]`);
  lines.push(`  output extensionsRequired: ${JSON.stringify(r.extensionsRequired)}`);
  return lines.join('\n');
}
const fmt = (v) => (typeof v === 'number' ? String(v) : JSON.stringify(v));

// ─── CLI ────────────────────────────────────────────────────────────────────

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const j = args.indexOf('--json');
  const jsonOut = j >= 0 ? args.splice(j, 2)[1] : null;
  const [source, ...outputs] = args;
  if (!source || !outputs.length) {
    console.error('usage: node layer-extensions.mjs <source> <output> [<output> ...] [--json <file>]');
    process.exit(2);
  }
  try {
    usePixelDigest(await (await import('./pixel-digest.mjs')).loadPixelDigest());
  } catch {
    // No transcoder (no `npm ci`): a KTX2 image is compared by bytes alone.
  }
  const results = outputs.map((o) => compareFiles(source, o));
  for (const r of results) console.log(formatResult(r));
  if (jsonOut) fs.writeFileSync(jsonOut, JSON.stringify(results, null, 1));
  process.exit(results.every((r) => r.verdict === 'pass') ? 0 : 1);
}
