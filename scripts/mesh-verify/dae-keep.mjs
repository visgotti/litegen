// The `dae-keep` variant's pure rules (mesh-gaps item 8, Collada morph output),
// shared by verify.mjs and its tests (dae-keep.test.mjs).
//
// `mesh_matrix --dae-keep` writes, for every direction into Collada, a second
// file with `WriteOptions.dae_morphs = Keep`: the case's `to` is `dae-keep` in
// the manifest, and verify.mjs reads it as `dae` for every judge that reads
// Collada. Two readers are expected to REFUSE a file that holds a `<morph>`
// (decided per file, `hasMorph`, never by the case's name), and xmllint judges
// every such file against the COLLADA 1.4.1 schema.
import { createHash } from 'node:crypto';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { SCENEKIT_SHARED_CLIPS } from './scenekit-limits.mjs';

const exec = promisify(execFile);

export const DAE_KEEP = 'dae-keep';

// A file is expected to be refused by the readers that cannot play morphs when
// it contains a `<morph>` element. A fixture with no targets writes the same
// file under `keep` as under `bake`: that is an ordinary Collada file.
export const hasMorph = (text) => /<morph[\s/>]/.test(String(text).replace(/<!--[\s\S]*?-->/g, ''));

// ─── The two expected refusals ────────────────────────────────────────────────

// three.js r183 (0.183.2) ColladaLoader. `parseController` stores a `<morph>`
// controller with no `skin` (collada/ColladaParser.js:445-448, which only warns
// "Morph target animation not supported yet"), then `buildNode` reads
// `controller.skin.joints` (collada/ColladaComposer.js:2353) and throws; a skin
// over a morph throws earlier, in `buildController` reading the morph
// controller's id as a geometry (ColladaComposer.js:961-969, `geometry.sources`).
// A morph with a weight animation throws earlier still, in the parser: the
// weight channel's target is `<morph id>-weights(i)` (the form SceneKit plays,
// spike findings 1), which has no `/`, so `parseAnimationChannel` takes
// `sid = parts.shift()` as undefined and reads `sid.indexOf` (ColladaParser.js:
// 347-354). Measured on 2026-11-09 (the morph fixture, and the spike's files
// in a Chromium page): an animated morph fails there, a static one at `joints`
// (stack line ColladaComposer.js:2353) and a static skin over a morph at
// `sources` (:969).
export const THREE_KEEP_CITE =
  "three.js r183 ColladaLoader fails the whole file on a <morph>: ColladaParser.parseController stores a morph controller with no `skin` (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/collada/ColladaParser.js:445-448, a console.warn \"Morph target animation not supported yet\"), then ColladaComposer.buildNode reads `controller.skin.joints` (collada/ColladaComposer.js:2353) and a skin over a morph fails in buildController reading the morph controller's id as a geometry (ColladaComposer.js:961-969); a weight animation fails earlier, in ColladaParser.parseAnimationChannel (ColladaParser.js:347-354): its target `<morph id>-weights(i)` has no `/`, so `sid` is undefined and `sid.indexOf` throws. A TypeError on 'joints', 'sources', or 'indexOf' in parseAnimationChannel";
const THREE_KEEP_ERRORS = [
  /Cannot read properties of undefined \(reading '(?:joints|sources)'\)/,
  /Cannot read properties of undefined \(reading 'indexOf'\)[\s\S]*parseAnimationChannel/,
];

// assimp v6.0.5: ColladaParser::ReadController's `<targets>` loop starts from
// the controller's first child and advances `currentNode`, not
// `currentChildNode` (code/AssetLib/Collada/ColladaParser.cpp:902-914, the loop
// at :903), so `mMorphTarget` stays "" and ColladaLoader resolves it
// (code/AssetLib/Collada/ColladaLoader.cpp:685) with ResolveLibraryReference
// (ColladaParser.h:309-312): `Unable to resolve library reference ""`.
export const ASSIMP_KEEP_CITE =
  "assimp v6.0.5 cannot import a Collada <morph>: ColladaParser::ReadController's <targets> loop advances the wrong node (code/AssetLib/Collada/ColladaParser.cpp:902-914, the loop at :903), so mMorphTarget stays \"\" and ColladaLoader.cpp:685 fails resolving it, 'Unable to resolve library reference \"\"' (ColladaParser.h:309-312)";
const ASSIMP_KEEP_ERROR = /Unable to resolve library reference ""/;

const NO_LONGER_HOLDS = 'documented reader limitation no longer holds';

// `error`: the string the page returns when `ColladaLoader` throws
// (`renderCase`'s `error`), undefined when it loaded.
export function threeKeepVerdict(error) {
  if (error === undefined || error === null || error === '') {
    return { refused: false, fail: `${NO_LONGER_HOLDS}: three.js ColladaLoader loaded a file with a <morph> (${THREE_KEEP_CITE}); update the docs` };
  }
  if (!THREE_KEEP_ERRORS.some((re) => re.test(error))) {
    return { refused: false, fail: `three.js refused the <morph> file for another reason than the documented one: ${String(error).slice(0, 300)}` };
  }
  return { refused: true, cite: THREE_KEEP_CITE, error: String(error).slice(0, 300) };
}

// `validation`: validateAssimp's record for the file.
export function assimpKeepVerdict(validation) {
  if (validation?.ok) {
    return { refused: false, fail: `${NO_LONGER_HOLDS}: assimp read a file with a <morph> (${ASSIMP_KEEP_CITE}); update the docs` };
  }
  const messages = (validation?.messages ?? []).join(' ');
  if (!ASSIMP_KEEP_ERROR.test(messages)) {
    return { refused: false, fail: `assimp refused the <morph> file for another reason than the documented one: ${messages.slice(0, 300)}` };
  }
  return { refused: true, cite: ASSIMP_KEEP_CITE, error: messages.slice(0, 300) };
}

// ─── The xmllint pass ─────────────────────────────────────────────────────────
//
// COLLADA 1.4.1's schema is Khronos's, fetched when the harness runs into a
// cache outside the repo and never committed. Each download goes into a new,
// empty folder, its sha-256 is checked against the pin below, and only then is
// the folder kept; a file that does not match is refused, not used. The
// schema imports W3C's xml.xsd by URL (line 22 of the 1.4.1 file, for
// `xml:base`), which xmllint would fetch over the network: it is fetched and
// pinned the same way, and an XML catalog maps the URL to the local copy, with
// `--nonet` so nothing else is fetched.
export const SCHEMA_PINS = {
  collada: {
    file: 'collada_schema_1_4_1.xsd',
    url: 'https://www.khronos.org/files/collada_schema_1_4_1.xsd',
    sha256: '49266165027329388e710ade6a424116570cda6776cbf3623f7b8c78c727b0ff',
  },
  xml: {
    file: 'xml.xsd',
    url: 'https://www.w3.org/2001/03/xml.xsd',
    sha256: '0ed94db44402526e7a683424d0bd4b65e5bc50e65d9b9a762b11b30bae06d0c8',
  },
};
// The location the Collada schema imports it from.
export const XML_XSD_IMPORT = 'http://www.w3.org/2001/03/xml.xsd';

export function defaultCacheRoot(env = process.env, home = os.homedir()) {
  if (env.MESH_VERIFY_CACHE) return env.MESH_VERIFY_CACHE;
  return path.join(env.XDG_CACHE_HOME || path.join(home, '.cache'), 'litegen-mesh-verify');
}

export function catalogText(xmlXsdPath) {
  const uri = pathToFileURL(xmlXsdPath).href;
  return `<?xml version="1.0"?>\n<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">\n  <system systemId="${XML_XSD_IMPORT}" uri="${uri}"/>\n</catalog>\n`;
}

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

async function fetchHttps(url) {
  const res = await fetch(url, { signal: AbortSignal.timeout(60000) });
  if (!res.ok) throw new Error(`${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

// One pinned file, in `<cacheRoot>/<file>-<sha256>/<file>`. Returns
// { path } | { refused } | { skipped }.
async function pinnedFile(cacheRoot, pin, fetchBytes) {
  const dir = path.join(cacheRoot, `${pin.file}-${pin.sha256}`);
  const target = path.join(dir, pin.file);
  const have = await fs.readFile(target).catch(() => null);
  if (have) {
    const got = sha256(have);
    if (got !== pin.sha256) {
      return { refused: `${target} has sha-256 ${got}, not the pinned ${pin.sha256}: refused, not used (delete ${dir} to fetch it again)` };
    }
    return { path: target };
  }
  let bytes;
  try {
    bytes = await fetchBytes(pin.url);
  } catch (e) {
    return { skipped: `could not fetch ${pin.url}: ${e.message || e}` };
  }
  const got = sha256(bytes);
  if (got !== pin.sha256) {
    return { refused: `${pin.url} has sha-256 ${got}, not the pinned ${pin.sha256}: refused, not used (${pin.file} changed; review it and update the pin in dae-keep.mjs)` };
  }
  // Its own new, empty folder, kept only once the pin matched. A cache that
  // cannot be written skips the pass, saying why, as an unfetchable schema does.
  let tmp;
  try {
    await fs.mkdir(cacheRoot, { recursive: true });
    tmp = await fs.mkdtemp(path.join(cacheRoot, 'dl-'));
    await fs.writeFile(path.join(tmp, pin.file), bytes);
    await fs.rename(tmp, dir).catch(async (e) => {
      // Another run kept the same file first: that one is the same bytes.
      if (!(await fs.stat(target).catch(() => null))) throw e;
    });
  } catch (e) {
    return { skipped: `could not write ${pin.file} into the cache ${cacheRoot}: ${e.message || e}` };
  } finally {
    if (tmp) await fs.rm(tmp, { recursive: true, force: true });
  }
  return { path: target };
}

// { status: 'ready', xsd, catalog } | { status: 'refused' | 'skipped', reason }.
// A refused file (changed on its server, or in the cache) fails the pass; a
// schema that cannot be fetched at all skips it, saying why, as a missing tool
// does.
export async function prepareSchema({ cacheRoot = defaultCacheRoot(), pins = SCHEMA_PINS, fetchBytes = fetchHttps } = {}) {
  const got = {};
  for (const [name, pin] of Object.entries(pins)) {
    const r = await pinnedFile(cacheRoot, pin, fetchBytes);
    if (r.refused) return { status: 'refused', reason: r.refused };
    if (r.skipped) return { status: 'skipped', reason: r.skipped };
    got[name] = r.path;
  }
  const catalog = path.join(cacheRoot, 'catalog.xml');
  try {
    await fs.mkdir(cacheRoot, { recursive: true });
    await fs.writeFile(catalog, catalogText(got.xml));
  } catch (e) {
    return { status: 'skipped', reason: `could not write the catalog ${catalog}: ${e.message || e}` };
  }
  return { status: 'ready', xsd: got.collada, catalog, pins };
}

// xmllint's verdict. Exit 0: valid. Exit 3 and 4: the file fails the schema.
// Exit 5: the schema itself did not compile, which says nothing about the
// file and must never read as a pass (`kind: 'schema'`).
export function parseXmllint({ code, stdout = '', stderr = '' }) {
  const lines = `${stdout}\n${stderr}`.split('\n').map((l) => l.trim()).filter(Boolean);
  if (code === 0) return { kind: 'valid', ok: true, errors: 0, messages: [] };
  const errors = lines.filter((l) => /validity error/.test(l));
  if (code === 5 || lines.some((l) => /failed to compile|Schemas parser error/.test(l))) {
    return { kind: 'schema', ok: false, errors: 0, messages: lines.slice(0, 6) };
  }
  return { kind: 'invalid', ok: false, errors: errors.length, messages: (errors.length ? errors : lines).slice(0, 6) };
}

export async function xmllintFile(file, prepared, { xmllint = 'xmllint' } = {}) {
  try {
    const { stdout, stderr } = await exec(xmllint, ['--nonet', '--noout', '--schema', prepared.xsd, file], {
      env: { ...process.env, XML_CATALOG_FILES: prepared.catalog },
      timeout: 120000,
      maxBuffer: 16 << 20,
    });
    return parseXmllint({ code: 0, stdout, stderr });
  } catch (e) {
    if (typeof e.code !== 'number') return { kind: 'schema', ok: false, errors: 0, messages: [`xmllint did not run: ${e.message}`] };
    return parseXmllint({ code: e.code, stdout: e.stdout, stderr: e.stderr });
  }
}

// ─── What must judge, and what must be in, a <morph> file (Ruling DM18) ──────

const KEEP_NO_JUDGE =
  "no judge of the geometry of a Collada file that holds a <morph>: three.js and assimp refuse it (as documented), so SceneKit's silhouette is the only one, and it must be judged at rest and at every sample time";

// `silhouette`: `entry.scenekit?.silhouette`, SceneKit's frames by key
// (`static` for the rest picture, each sample time as a string); `keys`: the
// frames the case renders. One frame skipped (SceneKit registers no clip named
// like a node, geometry or animation id: `sceneKitClip`'s `notPlayed`), one
// that errored and one never rendered each leave an animated morph judged at
// rest alone, so each fails the case.
//
// One limitation is accepted, cited and counted (Ruling DM21): when clips
// animate the same node SceneKit's importer binds keys by target and plays none
// of them (SCENEKIT_SHARED_CLIPS), so every animated frame is skipped, while
// the rest frame, still judged and passing, shows the default weights. Such a
// case passes with `animatedUnjudged` set to the citation, and the run counts
// those cases on their own (`daeKeepSummary`). Only that exact skip, on every
// animated frame: a clip SceneKit cannot register (`notPlayed`), an error, a
// missing frame or any other skip, alone or beside it, still fails.
export function keepSilhouetteVerdict(silhouette, keys) {
  if (!silhouette) return { fail: `${KEEP_NO_JUDGE}, and SceneKit did not run` };
  if (silhouette.skipped) return { fail: `${KEEP_NO_JUDGE}, and its silhouette was skipped: ${String(silhouette.skipped).slice(0, 200)}` };
  const animated = keys.filter((key) => key !== 'static');
  const sharedClips = animated.length > 0 && animated.every((key) => silhouette[key]?.skipped === SCENEKIT_SHARED_CLIPS);
  const unjudged = [];
  for (const key of keys) {
    if (sharedClips && key !== 'static') continue;
    const at = key === 'static' ? 'rest' : `t=${key}`;
    const frame = silhouette[key];
    if (!frame) unjudged.push(`${at} (not rendered)`);
    else if (frame.skipped) unjudged.push(`${at} (skipped: ${String(frame.skipped).slice(0, 200)})`);
    else if (frame.error) unjudged.push(`${at} (${String(frame.error).slice(0, 200)})`);
  }
  if (unjudged.length) return { fail: `${KEEP_NO_JUDGE}; it was not judged at ${unjudged.join(', ')}` };
  return sharedClips ? { ok: true, animatedUnjudged: SCENEKIT_SHARED_CLIPS } : { ok: true };
}

// A `dae-keep` file for an expected scene that has morph targets must hold a
// `<morph>`: a writer that regressed to baking would write an ordinary
// Collada file that every judge passes. Returns the failure, or null.
export const keepMissingMorph = (expectedMorphed, fileHasMorph) =>
  expectedMorphed && !fileHasMorph ? 'the expected scene has morph targets but the dae-keep file holds no <morph>' : null;

// `summary.tools`: the tools the run had, plus xmllint only when the run has
// `dae-keep` cases, the only ones that use it.
export const reportedTools = (tools, { daeKeepCases, xmllint }) => ({ ...tools, ...(daeKeepCases ? { xmllint } : {}) });

// The schema record `report.json` keeps: the cache's paths written from `~`,
// never from the home folder, so the report names no user.
export function schemaForReport(prepared, home = os.homedir()) {
  if (!prepared) return prepared;
  // The home folder only as a whole path segment: not `/home/someoneelse`.
  const homePath = home ? new RegExp(`${home.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}(?![\\w.~-])`, 'g') : null;
  const tilde = (text) => (typeof text === 'string' && homePath ? text.replace(homePath, '~') : text);
  const out = { ...prepared };
  for (const key of ['xsd', 'catalog', 'reason']) if (key in out) out[key] = tilde(out[key]);
  return out;
}

// The run's dae-keep summary line (verify.mjs prints it, `summary.daeKeep` holds
// the same totals). The cases with animated frames unjudged are counted on
// their own, so they cannot hide in the passes (Ruling DM21).
export function daeKeepSummary(t) {
  const x = t.xmllint;
  const pass = x.checked
    ? `${x.valid} of ${x.checked} validate against COLLADA 1.4.1, ${x.invalid} do not${x.schemaBroken ? `, ${x.schemaBroken} not judged (the schema did not compile)` : ''}`
    : x.refused
      ? `FAILED, ${x.refused} file(s) not judged: ${t.schema.reason}`
      : `skipped (${t.schema?.reason ?? 'no file'})`;
  return `dae-keep (item 8): ${t.cases} case(s), ${t.morphFiles} with a <morph>; three.js refused ${t.threeRefused} and assimp ${t.assimpRefused} of those, as documented; ${t.animatedUnjudged} case(s) with animated frames unjudged (SceneKit's shared clips; three.js refuses <morph>); xmllint: ${pass}`;
}

// ─── SceneKit's uv judge for a dae-keep file (Ruling DM20) ───────────────────

// The page's `compareNear` (page/render.js), on two RGBA8 pixel arrays of w x h:
// a pixel differs only if NO pixel of `want` within one of it matches, per
// channel within `tol`. Two renderers sample a texture with sub-texel offsets,
// so every sharp edge moves by a pixel, which this absorbs while a wrong
// texture, a flipped uv or a wrong colour, which differ everywhere, still fail.
// Kept here, node-side, to compare two SceneKit pictures with each other: the
// page compares a picture only with its expected pictures.
export function compareNearPixels(want, got, tol, w, h) {
  let covered = 0;
  let differ = 0;
  const at = (x, y) => (y * w + x) * 4;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const i = at(x, y);
      const cg = got[i + 3] > 0;
      const cw = want[i + 3] > 0;
      if (!cg && !cw) continue;
      covered++;
      let ok = false;
      for (let dy = -1; dy <= 1 && !ok; dy++) {
        for (let dx = -1; dx <= 1 && !ok; dx++) {
          const xx = x + dx;
          const yy = y + dy;
          if (xx < 0 || yy < 0 || xx >= w || yy >= h) continue;
          const j = at(xx, yy);
          if ((want[j + 3] > 0) !== cg) continue;
          if (!cg) {
            ok = true;
            break;
          }
          const d = Math.max(Math.abs(want[j] - got[i]), Math.abs(want[j + 1] - got[i + 1]), Math.abs(want[j + 2] - got[i + 2]));
          if (d <= tol) ok = true;
        }
      }
      if (!ok) differ++;
    }
  }
  return { covered, differ, ratio: covered ? differ / covered : 0 };
}

// What judges one SceneKit uv frame of a `dae-keep` file. Keep changes no
// texture coordinate: SceneKit draws the bake's and the keep file's uvs
// byte-identically. What the harness had never asked is SceneKit's own uv
// picture of a textured model against three.js's expected one, and for a file
// that holds a `<morph>` SceneKit is the only uv judge (three.js refuses it),
// where MorphPrimitivesTest and MorphStressTest differ 4.07 % and 5.20 % at rest
// against the 2 % limit. So:
//   - at rest the frame is compared with SceneKit's picture of the plain
//     `<from>__dae` counterpart (the bake, whose uvs three.js judged), at the
//     uv limit; the ratio against the expected picture stays beside it as
//     information. No counterpart: the expected picture;
//   - an animated frame is compared with the expected picture as for any file,
//     unless the counterpart's own ratio against the expected picture at rest
//     already exceeds the limit, which fails it; then the frame is skipped
//     with that ratio named, and the silhouette judges the animated geometry.
// `counterpart`: null, or { expectedRatio } (the counterpart's SceneKit uv
// picture at rest against the expected one). Returns { against: 'counterpart' |
// 'expected' } or { skip: <citation> }.
export function keepUvJudge({ rest, counterpart, limit }) {
  if (!counterpart) return { against: 'expected' };
  if (rest) return { against: 'counterpart' };
  if (counterpart.expectedRatio > limit) {
    const pct = (r) => `${(r * 100).toFixed(2)} %`;
    return {
      skip: `SceneKit's uv picture of the plain Collada file (the bake) is ${pct(counterpart.expectedRatio)} apart from the expected picture at rest, over the ${pct(limit)} limit: this model's textures draw differently in SceneKit and three.js whichever way the morphs are written, and SceneKit draws the bake and keep files' uvs identically at rest; the silhouette judges the animated geometry`,
    };
  }
  return { against: 'expected' };
}
