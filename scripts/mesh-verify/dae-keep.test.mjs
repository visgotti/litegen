// node --test scripts/mesh-verify/dae-keep.test.mjs
//
// The pure rules the `dae-keep` variant's judges share (verify.mjs): which files
// are expected to be refused, what a refusal must look like, and the xmllint
// pass with its pinned, offline-resolved schema.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {
  DAE_KEEP,
  SCHEMA_PINS,
  assimpKeepVerdict,
  catalogText,
  compareNearPixels,
  daeKeepSummary,
  defaultCacheRoot,
  hasMorph,
  keepMissingMorph,
  keepSilhouetteVerdict,
  keepUvJudge,
  parseXmllint,
  prepareSchema,
  reportedTools,
  schemaForReport,
  threeKeepVerdict,
  xmllintFile,
} from './dae-keep.mjs';
import { SCENEKIT_CLIP_SHADOWED, SCENEKIT_SHARED_CLIPS } from './scenekit-limits.mjs';

const run = promisify(execFile);
const sha = (b) => createHash('sha256').update(b).digest('hex');
const haveXmllint = await run('/usr/bin/which', ['xmllint']).then(() => true, () => false);

test('the manifest names the variant dae-keep', () => {
  assert.equal(DAE_KEEP, 'dae-keep');
});

test('a file is expected to be refused when it contains a <morph> element, not by its case name', () => {
  assert.equal(hasMorph('<controller id="m"><morph method="NORMALIZED" source="#g"/></controller>'), true);
  assert.equal(hasMorph('<morph\n source="#g">'), true);
  assert.equal(hasMorph('<COLLADA><library_geometries/></COLLADA>'), false);
  assert.equal(hasMorph('<skin source="#g"/><morphology/>'), false, 'another element that starts with the letters');
  assert.equal(hasMorph('<!-- <morph source="#g"/> --><COLLADA/>'), false, 'a comment is not an element');
});

test('three.js must fail on the documented TypeError, and a load or another failure fails the case', () => {
  const joints = "TypeError: Cannot read properties of undefined (reading 'joints') | at Object.buildNode (http://127.0.0.1:1/x.js:1:1)";
  const sources = "TypeError: Cannot read properties of undefined (reading 'sources') | at buildController";
  assert.equal(threeKeepVerdict(joints).refused, true);
  assert.equal(threeKeepVerdict(sources).refused, true);
  assert.match(threeKeepVerdict(joints).cite, /ColladaComposer\.js:2353/);
  assert.match(threeKeepVerdict(joints).cite, /ColladaParser\.js:445-448/);
  // An animated morph fails earlier, in the parser: the weight channel's target
  // (`<morph id>-weights(0)`, the form SceneKit plays) has no `/`, so
  // parseAnimationChannel reads `sid.indexOf` of undefined (measured, r183).
  const channel =
    "TypeError: Cannot read properties of undefined (reading 'indexOf') |     at ColladaParser.parseAnimationChannel (http://127.0.0.1:1/three/examples/jsm/loaders/collada/ColladaParser.js:354:29) |     at ColladaParser.parseAnimation (http://127.0.0.1:1/x.js:290:9)";
  assert.equal(threeKeepVerdict(channel).refused, true);
  assert.match(threeKeepVerdict(channel).cite, /ColladaParser\.js:347-354/);
  const strayIndexOf = threeKeepVerdict("TypeError: Cannot read properties of undefined (reading 'indexOf') |     at somewhere.else (x.js:1:1)");
  assert.equal(strayIndexOf.refused, false, 'an indexOf failure elsewhere is not the documented one');
  const loaded = threeKeepVerdict(undefined);
  assert.equal(loaded.refused, false);
  assert.match(loaded.fail, /documented reader limitation no longer holds/);
  assert.match(loaded.fail, /three\.js/);
  const other = threeKeepVerdict('Error: failed to load http://127.0.0.1:1/model.dae: 404');
  assert.equal(other.refused, false);
  assert.match(other.fail, /another reason/);
  assert.match(other.fail, /404/);
});

test('assimp must refuse with the documented message; a pass or another failure fails the case', () => {
  const refused = {
    tool: 'assimp',
    ok: false,
    messages: ['Command failed', '0 %50 %ERROR: Failed to load file: Unable to resolve library reference "".'],
  };
  const v = assimpKeepVerdict(refused);
  assert.equal(v.refused, true);
  assert.match(v.cite, /ColladaParser\.cpp:902-914/);
  assert.match(v.cite, /ColladaLoader\.cpp:685/);
  const read = assimpKeepVerdict({ tool: 'assimp', ok: true, meshes: 1 });
  assert.equal(read.refused, false);
  assert.match(read.fail, /documented reader limitation no longer holds/);
  assert.match(read.fail, /assimp/);
  const other = assimpKeepVerdict({ tool: 'assimp', ok: false, messages: ['ERROR: Failed to load file: Mesh contains no faces'] });
  assert.equal(other.refused, false);
  assert.match(other.fail, /another reason/);
});

test('xmllint output is parsed into valid, invalid (with a count) and a schema that did not compile', () => {
  const ok = parseXmllint({ code: 0, stdout: '', stderr: '/x/model.dae validates\n' });
  assert.deepEqual({ kind: ok.kind, ok: ok.ok, errors: ok.errors }, { kind: 'valid', ok: true, errors: 0 });
  const bad = parseXmllint({
    code: 3,
    stdout: '',
    stderr:
      "/x/model.dae:12: element morph: Schemas validity error : Element '{ns}morph': Missing child element(s). Expected is ( {ns}source ).\n" +
      "/x/model.dae:20: element targets: Schemas validity error : Element '{ns}targets': Missing child element(s). Expected is ( {ns}input ).\n" +
      '/x/model.dae fails to validate\n',
  });
  assert.equal(bad.kind, 'invalid');
  assert.equal(bad.ok, false);
  assert.equal(bad.errors, 2);
  assert.match(bad.messages[0], /Missing child element/);
  const broken = parseXmllint({
    code: 5,
    stdout: '',
    stderr: "schema.xsd:201: element attribute: Schemas parser error : attribute use (unknown), attribute 'ref': The QName value does not resolve.\nWXS schema schema.xsd failed to compile\n",
  });
  assert.equal(broken.kind, 'schema');
  assert.equal(broken.ok, false);
});

test('the catalog maps the schema import to the local copy', () => {
  const text = catalogText('/cache/xml.xsd-abc/xml.xsd');
  assert.match(text, /systemId="http:\/\/www\.w3\.org\/2001\/03\/xml\.xsd"/);
  assert.match(text, /uri="file:\/\/\/cache\/xml\.xsd-abc\/xml\.xsd"/);
  assert.match(catalogText('/a b/xml.xsd'), /uri="file:\/\/\/a%20b\/xml\.xsd"/);
});

test('the real pins name Khronos 1.4.1 and the W3C xml.xsd it imports, each by sha-256', () => {
  assert.match(SCHEMA_PINS.collada.url, /^https:\/\/www\.khronos\.org\/files\/collada_schema_1_4_1\.xsd$/);
  assert.match(SCHEMA_PINS.collada.sha256, /^[0-9a-f]{64}$/);
  assert.equal(SCHEMA_PINS.xml.url, 'https://www.w3.org/2001/03/xml.xsd');
  assert.match(SCHEMA_PINS.xml.sha256, /^[0-9a-f]{64}$/);
});

test('the cache is outside the repo and can be moved by an environment variable', () => {
  assert.equal(defaultCacheRoot({ MESH_VERIFY_CACHE: '/somewhere/cache' }, '/home/u'), '/somewhere/cache');
  assert.equal(defaultCacheRoot({ XDG_CACHE_HOME: '/xdg' }, '/home/u'), '/xdg/litegen-mesh-verify');
  assert.equal(defaultCacheRoot({}, '/home/u'), '/home/u/.cache/litegen-mesh-verify');
});

// A tiny schema that imports xml.xsd by URL, as the Collada schema does, and
// a stand-in xml.xsd: pins computed from these bytes.
const XML_XSD = Buffer.from(
  '<?xml version="1.0"?><xs:schema targetNamespace="http://www.w3.org/XML/1998/namespace" xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:attribute name="base" type="xs:anyURI"/></xs:schema>',
);
const MAIN_XSD = Buffer.from(
  '<?xml version="1.0"?><xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xml="http://www.w3.org/XML/1998/namespace">' +
    '<xs:import namespace="http://www.w3.org/XML/1998/namespace" schemaLocation="http://www.w3.org/2001/03/xml.xsd"/>' +
    '<xs:element name="a"><xs:complexType><xs:attribute ref="xml:base"/></xs:complexType></xs:element></xs:schema>',
);
const pins = {
  collada: { file: 'collada_schema_1_4_1.xsd', url: 'https://example.test/main.xsd', sha256: sha(MAIN_XSD) },
  xml: { file: 'xml.xsd', url: 'https://example.test/xml.xsd', sha256: sha(XML_XSD) },
};
const serve = (calls, override = {}) => async (url) => {
  calls.push(url);
  if (url in override) return override[url];
  if (url === pins.collada.url) return MAIN_XSD;
  if (url === pins.xml.url) return XML_XSD;
  throw new Error(`unexpected ${url}`);
};
// Every scratch folder a test makes is removed when the file's tests are done.
const made = [];
const scratch = async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'dae-keep-test-'));
  made.push(dir);
  return dir;
};
after(async () => {
  for (const dir of made) await fs.rm(dir, { recursive: true, force: true });
});

test('the schema is fetched once, each file into a folder of its own, and the second run uses the cache', async () => {
  const root = await scratch();
  const calls = [];
  const first = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve(calls) });
  assert.equal(first.status, 'ready');
  assert.deepEqual(await fs.readFile(first.xsd), MAIN_XSD);
  assert.equal(calls.length, 2);
  for (const pin of Object.values(pins)) {
    const dir = path.join(root, `${pin.file}-${pin.sha256}`);
    assert.deepEqual(await fs.readdir(dir), [pin.file], 'one file in its own folder');
  }
  assert.deepEqual((await fs.readdir(root)).filter((n) => n.startsWith('dl-')), [], 'no download folder is left behind');
  const again = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve(calls) });
  assert.equal(again.status, 'ready');
  assert.equal(calls.length, 2, 'nothing fetched the second time');
});

test('a download whose sha-256 is not the pin is refused and never kept', async () => {
  const root = await scratch();
  const r = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve([], { [pins.collada.url]: Buffer.from('<changed/>') }) });
  assert.equal(r.status, 'refused');
  assert.match(r.reason, /collada_schema_1_4_1\.xsd/);
  assert.match(r.reason, new RegExp(pins.collada.sha256));
  assert.match(r.reason, new RegExp(sha(Buffer.from('<changed/>'))));
  const left = await fs.readdir(root);
  assert.ok(!left.some((n) => n.startsWith(`${pins.collada.file}-`)), `kept ${left}`);
  assert.ok(!left.some((n) => n.startsWith('dl-')), `left a download folder ${left}`);
});

test('a cached file that no longer matches its pin is refused, not fetched again and not used', async () => {
  const root = await scratch();
  const calls = [];
  const first = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve(calls) });
  await fs.writeFile(first.xsd, 'tampered');
  const r = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve(calls) });
  assert.equal(r.status, 'refused');
  assert.match(r.reason, /delete/);
  assert.equal(calls.length, 2);
});

test('a schema that cannot be fetched skips the pass, saying why', async () => {
  const root = await scratch();
  const r = await prepareSchema({
    cacheRoot: root,
    pins,
    fetchBytes: async () => {
      throw new Error('getaddrinfo ENOTFOUND www.khronos.org');
    },
  });
  assert.equal(r.status, 'skipped');
  assert.match(r.reason, /ENOTFOUND/);
  assert.match(r.reason, /example\.test\/main\.xsd/);
});

test('xmllint judges a file against the pinned schema, resolving its import offline', { skip: !haveXmllint && 'xmllint is not installed' }, async () => {
  const root = await scratch();
  const prepared = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve([]) });
  assert.equal(prepared.status, 'ready');
  const good = path.join(root, 'good.xml');
  const bad = path.join(root, 'bad.xml');
  await fs.writeFile(good, '<a xmlns:xml="http://www.w3.org/XML/1998/namespace" xml:base="x"/>');
  await fs.writeFile(bad, '<b/>');
  const g = await xmllintFile(good, prepared);
  assert.equal(g.kind, 'valid', JSON.stringify(g));
  const b = await xmllintFile(bad, prepared);
  assert.equal(b.kind, 'invalid', JSON.stringify(b));
  assert.ok(b.errors >= 1);
  // Control: with no catalog the import is a network entity, and --nonet makes
  // the schema fail to compile. A schema that did not compile is not a verdict
  // on the file (kind 'schema'), so a missing catalog cannot pass a file.
  const withoutCatalog = await xmllintFile(good, { ...prepared, catalog: path.join(root, 'no-such-catalog.xml') });
  assert.equal(withoutCatalog.kind, 'schema', JSON.stringify(withoutCatalog));
  assert.equal(withoutCatalog.ok, false);
});

// ── the final fix wave (Rulings DM18 to DM20) ─────────────────────────────────

const judged = (over = {}) => ({ image: 'scenekit_silhouette.png', ratio: 0, ...over });

test('SceneKit must judge a <morph> file\'s silhouette at rest and at every sample time (DM18)', () => {
  const keys = ['static', '0.5', '1'];
  const all = { static: judged(), '0.5': judged(), 1: judged() };
  assert.deepEqual(keepSilhouetteVerdict(all, keys), { ok: true });
  // A clip SceneKit does not register (`notPlayed`, scenekit-limits.mjs) skips the
  // animated frames: judged at rest alone, an animated morph would pass.
  const notPlayed = SCENEKIT_CLIP_SHADOWED;
  const skipped = keepSilhouetteVerdict({ static: judged(), '0.5': { skipped: notPlayed }, 1: { skipped: notPlayed } }, keys);
  assert.equal(skipped.ok, undefined, 'a skipped frame is not a pass');
  assert.equal(skipped.animatedUnjudged, undefined);
  assert.match(skipped.fail, /every sample time/);
  assert.match(skipped.fail, /t=0\.5/);
  assert.match(skipped.fail, /t=1/);
  assert.match(skipped.fail, /registers an <animation_clip> under its name/);
  assert.doesNotMatch(skipped.fail, /rest \(/, 'the rest frame was judged');
  // One skipped frame is enough, whichever it is.
  assert.match(keepSilhouetteVerdict({ ...all, static: { skipped: 'rest skip' } }, keys).fail, /rest \(skipped: rest skip\)/);
  assert.match(keepSilhouetteVerdict({ ...all, 1: { skipped: 'late skip' } }, keys).fail, /t=1 \(skipped: late skip\)/);
  // A frame that errored, and one that was never rendered, were not judged either.
  assert.match(keepSilhouetteVerdict({ ...all, '0.5': { error: 'render failed' } }, keys).fail, /render failed/);
  const missing = { static: judged() };
  assert.match(keepSilhouetteVerdict(missing, keys).fail, /t=0\.5 \(not rendered\)/);
  // The whole mode skipped, or SceneKit absent.
  assert.match(keepSilhouetteVerdict({ skipped: 'no silhouette for this format' }, keys).fail, /no silhouette for this format/);
  assert.match(keepSilhouetteVerdict(undefined, keys).fail, /SceneKit did not run/);
  // A fixture with no clip has the rest picture alone.
  assert.deepEqual(keepSilhouetteVerdict({ static: judged() }, ['static']), { ok: true });
  assert.match(keepSilhouetteVerdict({ static: { skipped: 'x' } }, ['static']).fail, /rest \(skipped: x\)/);
});

test('animated frames SceneKit skips for its shared-clip limitation are "unjudged, cited", counted apart (DM21)', () => {
  const keys = ['static', '0.5', '1'];
  const shared = { skipped: SCENEKIT_SHARED_CLIPS };
  // Clips animating one node replace each other's keys in SceneKit, which plays
  // none of them; three.js refuses the <morph> file. The rest frame, judged and
  // passing, is what remains, and the verdict carries the citation to count.
  const v = keepSilhouetteVerdict({ static: judged(), '0.5': shared, 1: shared }, keys);
  assert.deepEqual(v, { ok: true, animatedUnjudged: SCENEKIT_SHARED_CLIPS });
  // A file with all its frames judged carries no such mark.
  assert.deepEqual(keepSilhouetteVerdict({ static: judged(), '0.5': judged(), 1: judged() }, keys), { ok: true });
  // The rest frame must still be judged: skipped, errored or never rendered, it fails.
  assert.match(keepSilhouetteVerdict({ static: { skipped: 'rest skip' }, '0.5': shared, 1: shared }, keys).fail, /rest \(skipped: rest skip\)/);
  assert.match(keepSilhouetteVerdict({ static: { error: 'no frame' }, '0.5': shared, 1: shared }, keys).fail, /rest \(no frame\)/);
  assert.match(keepSilhouetteVerdict({ '0.5': shared, 1: shared }, keys).fail, /rest \(not rendered\)/);
  // Only that limitation is cited: a clip SceneKit cannot register (`notPlayed`),
  // a render error, a missing frame or any other skip still fails the case,
  // whatever the other animated frames are.
  const other = { skipped: 'some other reason' };
  assert.match(keepSilhouetteVerdict({ static: judged(), '0.5': shared, 1: { skipped: SCENEKIT_CLIP_SHADOWED } }, keys).fail, /t=1 \(skipped: SceneKit's Collada importer registers/);
  assert.match(keepSilhouetteVerdict({ static: judged(), '0.5': shared, 1: other }, keys).fail, /t=1 \(skipped: some other reason\)/);
  assert.match(keepSilhouetteVerdict({ static: judged(), '0.5': shared, 1: { error: 'render failed' } }, keys).fail, /t=1 \(render failed\)/);
  assert.match(keepSilhouetteVerdict({ static: judged(), '0.5': shared }, keys).fail, /t=1 \(not rendered\)/);
  // A mix of judged and shared-clip frames is not "all skipped for the limitation".
  assert.ok(keepSilhouetteVerdict({ static: judged(), '0.5': judged(), 1: shared }, keys).fail);
  // A fixture with no clip has nothing animated to leave unjudged.
  assert.deepEqual(keepSilhouetteVerdict({ static: judged() }, ['static']), { ok: true });
});

test('the dae-keep summary line counts the cases with animated frames unjudged on their own (DM21)', () => {
  const totals = (over = {}) => ({
    cases: 12,
    morphFiles: 7,
    threeRefused: 7,
    assimpRefused: 7,
    animatedUnjudged: 0,
    xmllint: { checked: 12, valid: 12, invalid: 0, schemaBroken: 0, skipped: 0, refused: 0 },
    ...over,
  });
  const line = daeKeepSummary(totals({ animatedUnjudged: 5 }));
  assert.match(line, /12 case\(s\), 7 with a <morph>/);
  assert.match(line, /three\.js refused 7 and assimp 7 of those, as documented/);
  assert.match(line, /xmllint: 12 of 12 validate against COLLADA 1\.4\.1, 0 do not/);
  assert.match(line, /5 case\(s\) with animated frames unjudged \(SceneKit's shared clips; three\.js refuses <morph>\)/);
  // The count is there when it is zero, so a run cannot hide one by saying nothing.
  assert.match(daeKeepSummary(totals()), /0 case\(s\) with animated frames unjudged/);
  // xmllint's other states are as they were.
  assert.match(daeKeepSummary(totals({ xmllint: { checked: 0, valid: 0, invalid: 0, schemaBroken: 0, skipped: 3, refused: 0 }, schema: { reason: 'xmllint is not installed' } })), /xmllint: skipped \(xmllint is not installed\)/);
  assert.match(daeKeepSummary(totals({ xmllint: { checked: 0, valid: 0, invalid: 0, schemaBroken: 0, skipped: 0, refused: 2 }, schema: { reason: 'sha mismatch' } })), /xmllint: FAILED, 2 file\(s\) not judged: sha mismatch/);
  assert.match(daeKeepSummary(totals({ xmllint: { checked: 10, valid: 8, invalid: 1, schemaBroken: 1, skipped: 0, refused: 0 } })), /8 of 10 validate against COLLADA 1\.4\.1, 1 do not, 1 not judged \(the schema did not compile\)/);
});

test('a dae-keep file for an expected scene with morph targets must hold a <morph> (DM18)', () => {
  const reason = 'the expected scene has morph targets but the dae-keep file holds no <morph>';
  assert.equal(keepMissingMorph(true, false), reason, 'a writer that baked its morphs');
  assert.equal(keepMissingMorph(true, true), null);
  assert.equal(keepMissingMorph(false, false), null, 'no targets, no morph: an ordinary Collada file');
  assert.equal(keepMissingMorph(undefined, false), null, 'an expected scene the page did not describe');
});

test('summary.tools reports xmllint only on a run with dae-keep cases', () => {
  const tools = { assimp: true, usdchecker: false, scenekit: true };
  assert.deepEqual(reportedTools(tools, { daeKeepCases: 0, xmllint: false }), tools, 'a plain run is as before');
  assert.ok(!('xmllint' in reportedTools(tools, { daeKeepCases: 0, xmllint: true })));
  assert.deepEqual(reportedTools(tools, { daeKeepCases: 3, xmllint: false }), { ...tools, xmllint: false });
  assert.deepEqual(reportedTools(tools, { daeKeepCases: 3, xmllint: true }), { ...tools, xmllint: true });
  assert.deepEqual(tools, { assimp: true, usdchecker: false, scenekit: true }, 'the input is not changed');
});

test('a cache folder that cannot be written skips the pass, saying why, instead of throwing', async () => {
  const root = await scratch();
  // A file where the cache folder should go: mkdir fails with ENOTDIR for anyone.
  const blocker = path.join(root, 'a-file');
  await fs.writeFile(blocker, 'not a folder');
  let r;
  await assert.doesNotReject(async () => {
    r = await prepareSchema({ cacheRoot: path.join(blocker, 'cache'), pins, fetchBytes: serve([]) });
  });
  assert.equal(r.status, 'skipped');
  assert.match(r.reason, /cache/);
  assert.match(r.reason, /ENOTDIR/);
});

test('a catalog that cannot be written skips the pass too', async () => {
  const root = await scratch();
  assert.equal((await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve([]) })).status, 'ready');
  // A folder where the catalog file goes: writeFile fails with EISDIR.
  await fs.rm(path.join(root, 'catalog.xml'));
  await fs.mkdir(path.join(root, 'catalog.xml'));
  let r;
  await assert.doesNotReject(async () => {
    r = await prepareSchema({ cacheRoot: root, pins, fetchBytes: serve([]) });
  });
  assert.equal(r.status, 'skipped');
  assert.match(r.reason, /catalog/);
  assert.match(r.reason, /EISDIR/);
});

test('the schema record in report.json names the cache with ~, not the home folder', () => {
  const home = '/home/someone';
  const cache = '/home/someone/.cache/litegen-mesh-verify';
  const ready = {
    status: 'ready',
    xsd: `${cache}/collada_schema_1_4_1.xsd-abc/collada_schema_1_4_1.xsd`,
    catalog: `${cache}/catalog.xml`,
    pins: { collada: { file: 'collada_schema_1_4_1.xsd', sha256: 'aa' } },
  };
  const before = JSON.stringify(ready);
  const record = schemaForReport(ready, home);
  assert.equal(record.xsd, '~/.cache/litegen-mesh-verify/collada_schema_1_4_1.xsd-abc/collada_schema_1_4_1.xsd');
  assert.equal(record.catalog, '~/.cache/litegen-mesh-verify/catalog.xml');
  assert.equal(record.status, 'ready');
  assert.deepEqual(record.pins, ready.pins);
  assert.ok(!JSON.stringify(record).includes(home));
  assert.equal(JSON.stringify(ready), before, 'the schema the run uses keeps its real paths');
  const refused = schemaForReport(
    { status: 'refused', reason: `${cache}/x/y has sha-256 aa, not the pinned bb: refused, not used (delete ${cache}/x to fetch it again)` },
    home,
  );
  assert.equal(refused.reason, '~/.cache/litegen-mesh-verify/x/y has sha-256 aa, not the pinned bb: refused, not used (delete ~/.cache/litegen-mesh-verify/x to fetch it again)');
  assert.equal(schemaForReport({ status: 'skipped', reason: 'cache /home/someoneelse/c' }, home).reason, 'cache /home/someoneelse/c', 'another folder that starts alike');
  assert.equal(schemaForReport({ status: 'skipped', reason: 'xmllint is not installed' }, home).reason, 'xmllint is not installed');
});

const rgba = (w, h, pixel) => {
  const out = new Uint8Array(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) out.set(pixel(x, y), (y * w + x) * 4);
  return out;
};
const grey = (v) => [v, v, v, 255];
const NONE = [0, 0, 0, 0];

test('the near comparison is the page\'s compareNear: a pixel differs only if no pixel within one of it matches', () => {
  const [w, h] = [8, 8];
  const flat = rgba(w, h, () => grey(100));
  const same = compareNearPixels(flat, flat, 24, w, h);
  assert.deepEqual({ covered: same.covered, differ: same.differ, ratio: same.ratio }, { covered: 64, differ: 0, ratio: 0 });
  // The tolerance is per channel and inclusive.
  assert.equal(compareNearPixels(flat, rgba(w, h, () => grey(124)), 24, w, h).ratio, 0, '24 apart is within');
  assert.equal(compareNearPixels(flat, rgba(w, h, () => grey(125)), 24, w, h).ratio, 1, '25 apart is not');
  assert.equal(compareNearPixels(flat, rgba(w, h, () => [100, 100, 150, 255]), 24, w, h).ratio, 1, 'one channel is enough');
  // An edge that moved by a pixel is absorbed; a wrong texture is not.
  const edge = (at) => rgba(w, h, (x) => (x < at ? grey(50) : grey(250)));
  assert.equal(compareNearPixels(edge(4), edge(5), 24, w, h).differ, 0, 'one pixel of edge');
  // Flipped, only the two columns beside the edge find a match within a pixel.
  assert.equal(compareNearPixels(edge(4), rgba(w, h, (x) => (x < 4 ? grey(250) : grey(50))), 24, w, h).ratio, 0.75, 'a flipped texture');
  // A pixel drawn where the other side has nothing within one pixel differs.
  const lone = compareNearPixels(rgba(w, h, () => NONE), rgba(w, h, (x, y) => (x === 4 && y === 4 ? grey(9) : NONE)), 24, w, h);
  assert.deepEqual({ covered: lone.covered, differ: lone.differ, ratio: lone.ratio }, { covered: 1, differ: 1, ratio: 1 });
  // Nothing drawn on either side compares nothing.
  assert.deepEqual(compareNearPixels(rgba(w, h, () => NONE), rgba(w, h, () => NONE), 24, w, h), { covered: 0, differ: 0, ratio: 0 });
});

test('a dae-keep file\'s SceneKit uv frames: at rest against the plain counterpart, animated against the expected picture unless the counterpart fails at rest (DM20)', () => {
  const limit = 0.02;
  // At rest the judge is SceneKit's own picture of the plain Collada file,
  // whatever that picture's ratio against the expected one.
  assert.deepEqual(keepUvJudge({ rest: true, counterpart: { expectedRatio: 0.0407 }, limit }), { against: 'counterpart' });
  assert.deepEqual(keepUvJudge({ rest: true, counterpart: { expectedRatio: 0.01 }, limit }), { against: 'counterpart' });
  assert.deepEqual(keepUvJudge({ rest: true, counterpart: null, limit }), { against: 'expected' }, 'no counterpart: the expected picture');
  // Animated frames keep the expected picture where the counterpart agrees with it at rest...
  assert.deepEqual(keepUvJudge({ rest: false, counterpart: { expectedRatio: 0.01675 }, limit }), { against: 'expected' }, 'the morph fixture');
  assert.deepEqual(keepUvJudge({ rest: false, counterpart: { expectedRatio: 0.02 }, limit }), { against: 'expected' }, 'at the limit is not over it');
  assert.deepEqual(keepUvJudge({ rest: false, counterpart: null, limit }), { against: 'expected' });
  // ...and are skipped, with the ratio named, where it already exceeds the uv limit.
  const skip = keepUvJudge({ rest: false, counterpart: { expectedRatio: 0.0407 }, limit });
  assert.equal(skip.against, undefined);
  assert.match(skip.skip, /4\.07 %/);
  assert.match(skip.skip, /2\.00 %/);
  assert.match(skip.skip, /identically at rest/);
  assert.match(skip.skip, /silhouette/);
});
