// The pure rules of mesh-gaps item 6's render-harness checks (USDC, several
// clips in USD, unlit in USD), shared by verify.mjs and its tests
// (usd-variants.test.mjs), as dae-keep.mjs is item 8's.
//
// `mesh_matrix --usd-variants` writes, for every direction into USDZ, three more
// cases: `usdz-all` (every clip, a USDA root), `usdz-crate` (every clip, a
// USDC root, `usdz_root=usdc`) and `usdz-crate-first` (the default single clip,
// a USDC root). The case's `to` is that name in the manifest,
// and verify.mjs reads it as `usdz` for every judge, as it reads `dae-keep` as
// `dae`.
//
// What lives here:
//  - the USDC checks: usdcat's own verdict (exit 0, no warning) and the text
//    comparison between the USDA and the USDC of one source;
//  - the clips checks: an `Animation` variant set read from usdcat's text, the
//    session layer that selects one variant, what usdchecker reports per
//    variant, the sample times and ranges of each clip, and what three.js must
//    list for a multi-clip file (`three_js_lists_every_clip`);
//  - the unlit checks: the two-light invariance verdict on pictures (the unlit
//    pixels do not move with the light, equal the base colour, and a lit
//    control in the same frame does move).
import { compareNearPixels } from './dae-keep.mjs';

// ─── the manifest's two USDZ variants ─────────────────────────────────────────

export const USDZ_ALL = 'usdz-all';
export const USDZ_CRATE = 'usdz-crate';
// A USDC root with the default single clip: the plain crate-rooted USDZ a device
// check needs (the two above keep every clip).
export const USDZ_CRATE_FIRST = 'usdz-crate-first';
export const USD_VARIANTS = [USDZ_ALL, USDZ_CRATE, USDZ_CRATE_FIRST];

// A manifest case whose `to` is one of the variants, as the usdz case every
// judge reads, with `variant` naming it; null for any other case.
export const usdVariantCase = (raw) => (USD_VARIANTS.includes(raw.to) ? { ...raw, to: 'usdz', variant: raw.to } : null);

// Whether the case's root layer is a crate: a .usdc, or a USDZ written with a
// USDC root.
export const crateRooted = (c) => c.to === 'usdc' || c.variant === USDZ_CRATE || c.variant === USDZ_CRATE_FIRST;

// ─── the Animation variant set ────────────────────────────────────────────────

// The text of a root layer (usdcat prints one for a .usda, .usdc or a USDZ's
// first entry) up to the end of the `variantSet "Animation"` body, read as the
// layout litegen writes (Apple's: spec "Several clips"): the default prim, the
// authored selection, and the variants in order. A scan that skips strings and
// comments, so a name in a string or a comment is no variant.
export function animationVariants(text) {
  const root = (String(text).match(/\bdefaultPrim\s*=\s*"([^"]+)"/) || [])[1] ?? null;
  const set = /\bvariantSet\s+"Animation"\s*=\s*\{/.exec(text);
  if (!set) return { root, selected: null, names: [] };
  const selected = (text.slice(0, set.index).match(/\bstring\s+Animation\s*=\s*"([^"]*)"/) || [])[1] ?? null;
  const names = [];
  let depth = 1;
  let i = set.index + set[0].length;
  while (i < text.length && depth > 0) {
    const ch = text[i];
    if (ch === '#') {
      while (i < text.length && text[i] !== '\n') i++;
    } else if (ch === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === '\\' ? 2 : 1;
      const word = text.slice(i + 1, j);
      i = j + 1;
      if (depth === 1) {
        let k = i;
        while (k < text.length && /\s/.test(text[k])) k++;
        if (text[k] === '{' || text[k] === '(') names.push(word);
      }
    } else {
      if (ch === '{') depth++;
      else if (ch === '}') depth--;
      i++;
    }
  }
  return { root, selected, names };
}

// A file whose expected scene has several clips holds them as variants: one with
// fewer than two is a writer that lost the variant set, not a file with nothing
// to check (the 2026-11-10 follow-up of item 6's final review). The expected
// scene is what `prepare` leaves for the file's options, so a USDZ that keeps
// its first clip expects one. The reason to fail it, or null.
export function missingVariantsReason(found, clipNames) {
  if (found.names.length >= 2 || clipNames.length < 2) return null;
  const has = found.names.length ? `only the variant "${found.names[0]}"` : 'no Animation variant set';
  return `the expected scene has ${clipNames.length} clips (${clipNames.join(', ')}), but the file has ${has}`;
}

// The variants a file holds against the expected scene's clips (`clipNames`, in
// the glTF's order). The names must be the glTF's clip names (only `clips()` has
// several today). usdcat prints variants in name order whatever order they were
// authored in, so the two are compared as sets; the authored selection must be
// the first clip's.
export function clipVariantsVerdict(found, clipNames) {
  const reasons = [];
  const [have, want] = [new Set(found.names), new Set(clipNames)];
  for (const n of found.names) if (!want.has(n)) reasons.push(`the file has a variant "${n}", which is no clip of the expected scene (${clipNames.join(', ')})`);
  for (const n of clipNames) if (!have.has(n)) reasons.push(`the expected scene's clip "${n}" has no variant in the file (${found.names.join(', ')})`);
  if (found.selected !== clipNames[0]) reasons.push(`the authored selection is ${JSON.stringify(found.selected)}, not the first clip "${clipNames[0]}"`);
  return { ok: reasons.length === 0, reasons };
}

// The session layer's selection of one variant: what usdrecord's
// `--sessionLayer` takes (research Q1, "Test without a device", step 2).
export function variantOver(root, name) {
  if (!/^[A-Za-z0-9_]+$/.test(root)) throw new Error(`prim name ${JSON.stringify(root)} is not an identifier`);
  if (!/^[A-Za-z0-9_][A-Za-z0-9_|-]*$/.test(name)) throw new Error(`variant name ${JSON.stringify(name)} is not a USD variant name`);
  return `over "${root}" (\n    variants = {\n        string Animation = "${name}"\n    }\n)\n{\n}\n`;
}

// ─── usdchecker, per variant ──────────────────────────────────────────────────

// `usdchecker` validates the stage with no variant selection, then each variant
// of the default prim's variant sets, and prints for each a header and its
// result (Apple USD Tools 0.25.2, measured on a two-clip file, plain and with
// --arkit alike):
//
//   Validation Result with no explicit variants set
//   Success!
//
//   Validation Result for Variants: Animation:bend on Prim: /Root
//   Success!
//
// and a last one for the empty selection (`Animation:`). A failing one lists
// its errors where `Success!` stands.
export function usdcheckerResults(output) {
  const lines = String(output).split('\n').map((l) => l.trim());
  const out = [];
  lines.forEach((line, i) => {
    let entry = null;
    if (line === 'Validation Result with no explicit variants set') entry = { variant: null, prim: null };
    else {
      const m = line.match(/^Validation Result for Variants: (.*) on Prim: (\S+)$/);
      if (m) entry = { variant: m[1].startsWith('Animation:') ? m[1].slice('Animation:'.length) : m[1], prim: m[2] };
    }
    if (!entry) return;
    const next = lines.slice(i + 1).find((l) => l !== '');
    out.push({ ...entry, ok: next === 'Success!' });
  });
  return out;
}

// Every variant the file holds must have been reported, and passed.
export function usdcheckerVariantVerdict(results, names) {
  const reasons = [];
  for (const name of names) {
    const r = results.find((x) => x.variant === name);
    if (!r) reasons.push(`usdchecker did not report variant "${name}"`);
    else if (!r.ok) reasons.push(`variant "${name}" failed usdchecker`);
  }
  return { ok: reasons.length === 0, reasons, reported: results.filter((r) => r.variant).map((r) => r.variant) };
}

// ─── clips: sample times and ranges ───────────────────────────────────────────

const SAMPLE_FRACTIONS = [0, 0.17, 0.36, 0.58, 0.83, 1];
// The times a clip is compared at: the same fractions of its own range as the
// fixtures' (fixtures.rs `sample_times`), deliberately not key times, in
// seconds to a millisecond (the manifest's rounding).
export const clipSampleTimes = (start, end) => SAMPLE_FRACTIONS.map((f) => Math.round((start + (end - start) * f) * 1000) / 1000);

// Each animation of a glTF: its first and last key, and the last key of its
// skeletal channels (a joint's translation, rotation or scale, which USD keeps
// in a SkelAnimation and three.js's USDLoader reads as that clip's tracks) and
// of its rigid ones (the same on a node that is no joint, which USD keeps as
// xformOp samples and three.js reads as `TransformAnimation`). A weights
// channel is neither: three.js reads no blendShapeWeights. glTF 2.0 §3.6.2.4
// requires min and max on a sampler input accessor, which is where the range
// is read.
export function clipRanges(json) {
  const joints = new Set((json.skins || []).flatMap((s) => s.joints || []));
  return (json.animations || []).map((a, index) => {
    let start = Infinity;
    let end = -Infinity;
    let skeletalEnd = null;
    let rigidEnd = null;
    for (const ch of a.channels || []) {
      const acc = json.accessors?.[a.samplers?.[ch.sampler]?.input];
      if (!acc?.min?.length || !acc?.max?.length) throw new Error(`animation ${a.name ?? index}: a sampler input has no min and max`);
      start = Math.min(start, acc.min[0]);
      end = Math.max(end, acc.max[0]);
      if (ch.target?.path === 'weights') continue;
      if (joints.has(ch.target?.node)) skeletalEnd = Math.max(skeletalEnd ?? -Infinity, acc.max[0]);
      else rigidEnd = Math.max(rigidEnd ?? -Infinity, acc.max[0]);
    }
    return { name: a.name ?? `animation_${index}`, start: Number.isFinite(start) ? start : 0, end: Number.isFinite(end) ? end : 0, skeletalEnd, rigidEnd };
  });
}

// ─── three.js: every clip (three_js_lists_every_clip) ────────────────────────

const THREE_TIME_TOLERANCE = 1e-3;
// USDComposer reads the rate as `framesPerSecond || timeCodesPerSecond || 30`
// (USDComposer.js:87), and USDAParser keeps only `upAxis` and `defaultPrim` of
// a layer's metadata (USDAParser.js:439-459), so a USDA is always read at 30:
// the spec's reason to judge three.js's durations on a USDC.
export const THREE_USDA_FPS =
  'three.js reads a USDA at 30 time codes per second whatever the file says (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDAParser.js:439-459 keeps only upAxis and defaultPrim of the layer metadata, and USDComposer.js:87 falls back to 30), so a clip\'s duration there is not the file\'s; judged on the USDC';

// What three.js's USDLoader lists for a file (`listed`: [{ name, duration }],
// the page's clips) against what the expected scene's clips are: `want.clips`
// each with the last key of its skeletal channels (`skeletalEnd`), and
// `want.rigidEnd`, the last key of the first clip's rigid channels, or null
// when it has none. USDLoader makes one clip per SkelAnimation prim, named
// after it (USDComposer.js:3586-3611), and one `TransformAnimation` for the
// xformOp samples (:3606-3611, the first clip's rigid motion, outside the
// variants). `crate`: the file's layer is a crate, where its durations are
// judged; a USDA's are a cited skip.
export function threeClipsVerdict(listed, want, { crate }) {
  const reasons = [];
  const skel = listed.filter((c) => c.name !== 'TransformAnimation');
  const transform = listed.filter((c) => c.name === 'TransformAnimation');
  const wanted = new Set(want.clips.map((c) => c.name));
  for (const clip of want.clips) {
    const found = skel.filter((c) => c.name === clip.name);
    if (found.length === 0) reasons.push(`three.js lists no clip named "${clip.name}"`);
    else if (found.length > 1) reasons.push(`three.js lists the clip "${clip.name}" ${found.length} times`);
    else if (crate && Math.abs(found[0].duration - clip.skeletalEnd) > THREE_TIME_TOLERANCE) {
      reasons.push(`clip "${clip.name}" lasts ${found[0].duration} s in three.js, the expected ${clip.skeletalEnd} s`);
    }
  }
  for (const c of skel) if (!wanted.has(c.name)) reasons.push(`three.js lists a clip the file should not have: "${c.name}"`);
  if (want.rigidEnd !== null && want.rigidEnd !== undefined) {
    if (transform.length !== 1) reasons.push(`three.js lists ${transform.length} TransformAnimation clip(s), the first clip's rigid motion needs one`);
    else if (crate && Math.abs(transform[0].duration - want.rigidEnd) > THREE_TIME_TOLERANCE) {
      reasons.push(`TransformAnimation lasts ${transform[0].duration} s in three.js, the first clip's rigid motion ends at ${want.rigidEnd} s`);
    }
  } else if (transform.length) {
    reasons.push('three.js lists a TransformAnimation, but no clip of the expected scene has rigid motion');
  }
  return { ok: reasons.length === 0, reasons, durations: crate ? { judged: true } : { judged: false, skipped: THREE_USDA_FPS } };
}

// ─── usdcat ───────────────────────────────────────────────────────────────────

// `usdcat model.usdc` exits 0, with no warning (spec "USDC: independent tools").
// A warning is anything on stderr.
export function usdcatVerdict({ code, stdout, stderr }) {
  const reasons = [];
  const first = (s) => String(s).trim().split('\n')[0].slice(0, 300);
  if (code !== 0) reasons.push(`usdcat exited ${code}: ${first(stderr)}`);
  else if (String(stderr).trim()) reasons.push(`usdcat printed a warning: ${first(stderr)}`);
  else if (!String(stdout).trim()) reasons.push('usdcat printed nothing: the file is not a layer');
  return { ok: reasons.length === 0, reasons };
}

// The attributes of a layer (usdcat's text) that hold an empty array, as a
// default or as a time sample. A crate stores such an array as a ValueRep with
// the array bit and payload 0 (OpenUSD v26.11.1, pxr/usd/sdf/crateFile.cpp
// L1775-L1777 packs it, L1805-L1808 unpacks it), which three.js's USDCParser
// does not treat as empty: it seeks to offset 0 and reads the file's magic as
// the array's size (USDCParser.js L1087-L1102, _readValue), so it throws or
// reads garbage on any crate with one.
export function crateEmptyArrays(text) {
  const names = [];
  let samples = null;
  const add = (n) => {
    if (!names.includes(n)) names.push(n);
  };
  for (const line of String(text).split('\n')) {
    const attr = line.match(/^\s*(?:custom\s+)?(?:uniform\s+)?[\w:]+\[\]\s+([\w:.]+?)(\.timeSamples)?\s*=\s*(\[\]|\{)\s*$/);
    if (attr) {
      if (attr[3] === '[]') add(attr[1]);
      else samples = attr[1];
      continue;
    }
    if (samples && /^\s*[-+\d.eE]+:\s*\[\],?\s*$/.test(line)) add(samples);
    else if (samples && /^\s*\}\s*$/.test(line)) samples = null;
  }
  return { count: names.length, names };
}

// usdcat prints a USDA and a USDC through one canonical printer, so two files
// of one source must print the same text, and any difference is a value, type,
// variability or metadata that the crate lost (spec; it replaces usddiff).
export function textDiff(a, b) {
  const lines = (t) => String(t).replace(/\n$/, '').split('\n');
  const [x, y] = [lines(a), lines(b)];
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    if (x[i] !== y[i]) return { equal: false, line: i + 1, a: x[i], b: y[i] };
  }
  return { equal: true, lines: x.length };
}

// ─── pictures ─────────────────────────────────────────────────────────────────

const channelDelta = (a, b, i, channels) => {
  let d = 0;
  for (let c = 0; c < channels; c++) d = Math.max(d, Math.abs(a[i + c] - b[i + c]));
  return d;
};

// Two RGBA pictures of one file (usdrecord of its USDA and of its USDC): the
// pixels either covers, and those where any channel, alpha included, differs by
// more than `tol`.
export function pixelsDiffer(a, b, tol) {
  if (a.length !== b.length) throw new Error(`the pictures differ in size: ${a.length / 4} and ${b.length / 4} pixels`);
  let pixels = 0;
  let differ = 0;
  let maxDelta = 0;
  for (let i = 0; i < a.length; i += 4) {
    if (a[i + 3] === 0 && b[i + 3] === 0) continue;
    pixels++;
    const d = channelDelta(a, b, i, 4);
    maxDelta = Math.max(maxDelta, d);
    if (d > tol) differ++;
  }
  return { pixels, differ, maxDelta };
}

// ─── unlit: the two-light check ──────────────────────────────────────────────

// How the check tells the unlit meshes from the lit control without trusting
// how a file encodes them: by names. The expected scene (GLTFLoader gives an
// unlit material a MeshBasicMaterial) lists, for each mesh, the names on its
// chain of ancestors (`chains`, each with `unlit`), and a name on both sides
// says nothing about either.
export function groupNames(chains) {
  const of = (flag) => [...new Set(chains.filter((c) => c.unlit === flag).flatMap((c) => c.names))];
  const [unlit, lit] = [of(true), of(false)];
  return { unlit: unlit.filter((n) => !lit.includes(n)), lit: lit.filter((n) => !unlit.includes(n)) };
}

// The Mesh prims of a flattened stage (`usdPrims`) that sit under, or are, a
// prim named one of `names`.
export function meshesOf(prims, names) {
  const wanted = new Set(names);
  return prims.filter((p) => p.type === 'Mesh' && p.path.split('/').some((seg) => wanted.has(seg))).map((p) => p.path);
}

export function coverageMask(rgba) {
  const mask = new Uint8Array(rgba.length / 4);
  for (let i = 0; i < mask.length; i++) mask[i] = rgba[i * 4 + 3] > 0 ? 1 : 0;
  return mask;
}

// Pixels only the unlit meshes cover, and only the lit control covers: where
// both cover, which of them is in front is not known here, so neither judges it.
export function pureMasks(unlitRgba, litRgba) {
  const [u, l] = [coverageMask(unlitRgba), coverageMask(litRgba)];
  return { unlit: u.map((x, i) => (x && !l[i] ? 1 : 0)), lit: l.map((x, i) => (x && !u[i] ? 1 : 0)) };
}

export const UNLIT_LIMITS = {
  // Per-channel levels (0-255) two pixels may differ by and still be the same
  // pixel: 8-bit rounding, the lit modes' tolerance (page/render.js, LIT_TOLERANCE).
  tolerance: 1,
  // "identical within 1 %": the share of the unlit pixels that may differ.
  invariance: 0.01,
  // The base colour, at the harness's unlit albedo tolerance (usdrecord's
  // albedo pass: 20/255 per channel, 3x3 neighbourhood, USD_THRESHOLD.albedo).
  albedoTolerance: 20,
  albedo: 0.08,
  // The lit control "must change": at least this share of its pixels moves by
  // more than the response tolerance when the light changes level.
  responseTolerance: 8,
  response: 0.5,
};

// Where the light changed: the unlit pixels (the mask), the same at two light
// levels. `meanDelta` and `maxDelta` are the residual: any highlight left on a
// surface that must not shade.
export function lightInvariance(a, b, mask, tol) {
  let pixels = 0;
  let differ = 0;
  let maxDelta = 0;
  let sum = 0;
  for (let p = 0; p < mask.length; p++) {
    if (!mask[p]) continue;
    pixels++;
    const d = channelDelta(a, b, p * 4, 4);
    sum += d;
    maxDelta = Math.max(maxDelta, d);
    if (d > tol) differ++;
  }
  return { pixels, differ, ratio: pixels ? differ / pixels : 0, maxDelta, meanDelta: pixels ? sum / pixels : 0 };
}

// The lit control: its pixels (the mask) that moved by more than `tol` when the
// light changed level.
export function lightResponse(a, b, mask, tol) {
  let pixels = 0;
  let moved = 0;
  let maxDelta = 0;
  for (let p = 0; p < mask.length; p++) {
    if (!mask[p]) continue;
    pixels++;
    const d = channelDelta(a, b, p * 4, 3);
    maxDelta = Math.max(maxDelta, d);
    if (d > tol) moved++;
  }
  return { pixels, moved, ratio: pixels ? moved / pixels : 0, maxDelta };
}

// The unlit pixels against the base colour (the expected albedo picture),
// inside the mask alone and with the neighbouring pixel allowed, as the
// albedo comparisons allow it (dae-keep.mjs `compareNearPixels`).
export function baseColourMatch(got, want, mask, tol, w, h) {
  const keep = (px) => {
    const out = px.slice();
    for (let p = 0; p < mask.length; p++) if (!mask[p]) out[p * 4 + 3] = 0;
    return out;
  };
  return compareNearPixels(keep(want), keep(got), tol, w, h);
}

const pct = (r) => `${(r * 100).toFixed(2)} %`;

// The verdict on one renderer's two-light pictures. `judgeBase`: whether the
// base colour is judged for this renderer or recorded only (a renderer
// measured to read the colour differently, cited by the caller).
export function unlitVerdict({ invariance, baseColour, response }, { judgeBase }) {
  const L = UNLIT_LIMITS;
  const reasons = [];
  if (!invariance.pixels) reasons.push('no unlit pixel in the frame to judge');
  else if (invariance.ratio > L.invariance) {
    reasons.push(`the unlit pixels change with the light: ${pct(invariance.ratio)} of ${invariance.pixels} differ by more than ${L.tolerance}/255 between the two light levels (max delta ${invariance.maxDelta}, mean ${invariance.meanDelta.toFixed(2)})`);
  }
  if (judgeBase && invariance.pixels && baseColour.ratio > L.albedo) {
    reasons.push(`the unlit pixels are not the base colour: ${pct(baseColour.ratio)} of ${baseColour.covered} differ from the expected albedo by more than ${L.albedoTolerance}/255`);
  }
  if (!response.pixels) reasons.push('no lit control pixel in the frame to judge');
  else if (response.ratio < L.response) {
    reasons.push(`the lit control did not change with the light: ${pct(response.ratio)} of ${response.pixels} pixels moved by more than ${L.responseTolerance}/255 (max delta ${response.maxDelta})`);
  }
  return { ok: reasons.length === 0, reasons };
}
