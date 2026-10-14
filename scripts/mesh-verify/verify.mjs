#!/usr/bin/env node
// Render-verify litegen's mesh conversion matrix.
//
//   cargo run --example mesh_matrix -- <matrix>     (in litegen-core)
//   node verify.mjs <matrix> <report> [--fixtures a,b] [--formats glb,obj]
//
// Every converted file is rendered by three.js's own loader for its format —
// the engine behind <model-viewer> and therefore behind the dashboard — and
// compared pixel by pixel with the EXPECTED picture: the fixture reduced to
// what the source and target containers can both hold (mesh::pose::degrade),
// written as GLB and rendered by GLTFLoader. glTF outputs are additionally run
// through the Khronos glTF-Validator, USD through Apple's usdchecker, and every
// format assimp can import through assimp, so a file that renders but is
// invalid still fails.
//
// Exit status is non-zero when any case fails.

import http from 'node:http';
import fs from 'node:fs/promises';
import fss from 'node:fs';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import validator from 'gltf-validator';

const run = promisify(execFile);
const here = path.dirname(fileURLToPath(import.meta.url));

const args = process.argv.slice(2);
const matrixDir = path.resolve(args[0] || '');
const outDir = path.resolve(args[1] || 'mesh-verify-report');
const opt = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const onlyFixtures = opt('--fixtures')?.split(',');
const onlyFormats = opt('--formats')?.split(',');
if (!args[0]) {
  console.error('usage: node verify.mjs <matrix_dir> <report_dir> [--fixtures a,b] [--formats glb,obj]');
  process.exit(2);
}

// Fraction of covered pixels allowed to differ, per mode. Rasterising the same
// triangles through different loaders is pixel-identical; what is allowed for
// is texture filtering at mip boundaries and 8-bit colour-space round trips.
const THRESHOLD = { geom: 0.01, normals: 0.02, albedo: 0.015, uv: 0.02 };

// ─── USD: a second, independent renderer ────────────────────────────────────
//
// three.js r183's USDLoader is a weak oracle for USD (page/render.js,
// `usdLimitation`, lists each defect with its source line): it reads linear
// UsdPreviewSurface colours as sRGB, keeps only displayColor[0], misreads every
// SkelAnimation rotation in USDA text, and ignores BlendShapes. Where it is
// wrong, these checks stand in, each against the same expected picture:
//
//  - usdrecord silhouettes (`usdrecord:cover`): Pixar's own Hydra Storm renders
//    the file through a camera equal to the harness camera, at the default
//    time and at every sample time; its alpha coverage is compared with the
//    expected picture's double-sided coverage. Proves positions, transforms,
//    winding-independent geometry, skinning, blend shapes and animation timing
//    as USD itself evaluates them. The camera lives in a SESSION layer: as a
//    wrapper root layer it would impose USD's default 24 time codes per second
//    on the file under test and shift every frame.
//  - usdrecord albedo (`usdrecord:albedo`): the same renders with every
//    UsdPreviewSurface's diffuse source routed to emission and all lighting and
//    reflectance off, so the image is base colour x texture x vertex colour as
//    Storm resolves the shading network (texture files, st primvars, V
//    orientation, colour spaces, factors, the displayColor primvar reader).
//  - three.js on a `usdcat` crate copy (`crate:geom`, skinned files only):
//    three.js reads USDC rotations correctly, so its skeletal animation of
//    Pixar's binary re-encoding of our text checks our skeleton through a
//    second, independent parser.
// Calibrated on correct files and on negative controls. Silhouettes of
// correct files match to 0.00% once a one-pixel edge shift is tolerated.
// Albedo of correct files differs on 0.2-5.7% of pixels, all along the
// textured fixture's one-texel stripes, which three.js minifies through
// mipmaps and Storm samples crisply; a wrong colour space differs on 98%, a
// mirrored texture on 82%.
const USD_THRESHOLD = { cover: 0.03, albedo: 0.08 };
const USD_FORMATS = { usdz: true, usda: true };

const manifest = JSON.parse(await fs.readFile(path.join(matrixDir, 'manifest.json'), 'utf8'));
const MIME = {
  '.html': 'text/html', '.js': 'text/javascript', '.mjs': 'text/javascript', '.json': 'application/json',
  '.png': 'image/png', '.jpg': 'image/jpeg', '.bin': 'application/octet-stream',
};

// ─── static server ──────────────────────────────────────────────────────────

const roots = {
  '/page/': path.join(here, 'page'),
  '/three/': path.join(here, 'node_modules', 'three'),
  '/m/': matrixDir,
  '/out/': outDir,
};
const server = http.createServer(async (req, res) => {
  const url = decodeURIComponent(new URL(req.url, 'http://x').pathname);
  for (const [prefix, dir] of Object.entries(roots)) {
    if (url.startsWith(prefix)) {
      const file = path.join(dir, url.slice(prefix.length));
      if (!file.startsWith(dir)) break;
      try {
        const body = await fs.readFile(file);
        res.writeHead(200, { 'content-type': MIME[path.extname(file)] || 'application/octet-stream' });
        res.end(body);
      } catch {
        res.writeHead(404);
        res.end();
      }
      return;
    }
  }
  res.writeHead(404);
  res.end();
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const base = `http://127.0.0.1:${server.address().port}`;

// ─── validators ─────────────────────────────────────────────────────────────

async function validateGltf(file) {
  const bytes = new Uint8Array(await fs.readFile(file));
  const dir = path.dirname(file);
  const report = await validator.validateBytes(bytes, {
    uri: path.basename(file),
    maxIssues: 50,
    externalResourceFunction: (uri) =>
      fs.readFile(path.join(dir, decodeURIComponent(uri))).then((b) => new Uint8Array(b)),
  });
  const issues = report.issues;
  return {
    tool: 'gltf-validator',
    ok: issues.numErrors === 0,
    errors: issues.numErrors,
    warnings: issues.numWarnings,
    messages: issues.messages
      .filter((m) => m.severity <= 1)
      .slice(0, 12)
      .map((m) => `${m.severity === 0 ? 'E' : 'W'} ${m.code} ${m.pointer || ''}: ${m.message}`),
  };
}

async function haveTool(name) {
  try {
    await run('/usr/bin/which', [name]);
    return true;
  } catch {
    return false;
  }
}
const TOOLS = {
  assimp: await haveTool('assimp'),
  usdchecker: await haveTool('usdchecker'),
  usdrecord: await haveTool('usdrecord'),
  usdcat: await haveTool('usdcat'),
};

// ─── SceneKit (second renderer, macOS only) ─────────────────────────────────
//
// Apple's SceneKit reads USD/USDZ, Collada, OBJ+MTL, PLY and STL through
// Model I/O. Built on demand from scenekit/render.swift; skipped silently
// elsewhere. Its pictures are compared with the SAME expected picture the
// three.js renders are, in two modes: silhouette (coverage — geometry,
// transforms, skinning, morphs, animation) and albedo (colour x texture x
// vertex colour), with a looser albedo tolerance because two renderers filter
// textures differently even when the data is identical.
const SCENEKIT_FORMATS = new Set(['obj', 'ply', 'stl', 'usdz', 'usda', 'dae']);
// Where Model I/O reads a file differently from every other consumer, the
// comparison would fail for a reason that is not ours. Each entry is a
// documented disagreement, not a tolerance.
// (Model I/O reads OBJ and PLY vertex colours as linear where three.js, MeshLab
// and Blender read sRGB; rather than skip those comparisons, the renderer is
// told the convention — `vertexColorsAreSRGB` in render.swift.)
//
// USD colour: SceneKit takes a UsdPreviewSurface colour value as linear
// Display P3, not linear sRGB (measured: linear (0.9, 0.3, 0.1) renders as
// (255, 143, 74), the exact P3-to-sRGB result, where (243, 149, 89) is right;
// Apple's own importer and packager output renders the same pixel), and it
// does not evaluate a primvar reader feeding diffuseColor (a vertex-coloured
// part draws in the colour authored beside the connection). usdrecord, USD's
// own renderer, judges USD albedo instead (usdOracle); SceneKit's USD
// silhouettes are compared as usual.
const SCENEKIT_USD_ALBEDO =
  'SceneKit reads UsdPreviewSurface colours as linear Display P3 and skips primvar readers; USD albedo is judged by usdrecord';
const SCENEKIT_SKIP = { 'usdz:albedo': SCENEKIT_USD_ALBEDO, 'usda:albedo': SCENEKIT_USD_ALBEDO };
// And SceneKit's USD importer sets blend-shape weights only through
// animation: a SkelAnimation's default `blendShapeWeights` is never applied
// (measured on the morph fixture: authored [0.25, 0], SCNMorpher weights
// [0, 0]), so with the clip removed for the rest pose every weight is 0.
// usdrecord, which evaluates the default time, judges that pose.
const SCENEKIT_USD_MORPH_DEFAULTS =
  "SceneKit's USD importer ignores a SkelAnimation's default blendShapeWeights; the default pose is judged by usdrecord";
// And it draws meshes under `over` prims. A prim is defined only when it and
// every ancestor are (`def` or `class`), and only defined prims are drawn, by
// usdrecord and by every Hydra renderer. Apple's own glTF importer keeps the
// prototype of an instanced mesh under such an `over` and references it
// (`over "_MeshPrototype_box" { def Mesh "box" }`), so SceneKit draws one
// extra, untransformed copy of it (measured: tp_textured_apple_usdz).
// And SceneKit's OBJ importer drops Kd when map_Kd is present (measured: an
// MTL at Kd 0.8 and at Kd 1 render identically), where the MTL spec, three.js
// and Blender multiply the map by Kd.
const SCENEKIT_OBJ_KD =
  "SceneKit ignores Kd beside map_Kd (the MTL spec multiplies them); albedo judged by three.js";
async function mtlTintsAMap(dir, files) {
  for (const f of files.filter((x) => x.toLowerCase().endsWith('.mtl'))) {
    const text = await fs.readFile(path.join(dir, f), 'utf8').catch(() => '');
    for (const block of text.split(/^newmtl\s/m).slice(1)) {
      const kd = (block.match(/^Kd\s+([^\n]+)/m) || [])[1];
      if (/^map_Kd\s/m.test(block) && kd && kd.trim().split(/\s+/).some((x) => Number(x) !== 1)) return true;
    }
  }
  return false;
}
const SCENEKIT_USD_UNDEFINED =
  "SceneKit draws meshes under undefined `over` prims (where Apple's importer keeps instancing prototypes); judged by usdrecord";
function usdMeshUnderOver(text) {
  const open = [];
  for (const line of text.split('\n')) {
    const m = line.match(/^(\s*)(def|over|class)\b(?:\s+\w+)?\s+"/);
    if (!m) continue;
    while (open.length && open[open.length - 1].indent >= m[1].length) open.pop();
    if (/^\s*def\s+Mesh\b/.test(line) && open.some((o) => o.over)) return true;
    open.push({ indent: m[1].length, over: m[2] === 'over' });
  }
  return false;
}

// Collada clips. SceneKit plays every animation in a file at once, so a file
// with several `<animation_clip>`s is played one clip at a time (`clip` in
// render.swift), the one three.js and the expected picture play. But
// SceneKit's importer binds animation data to its TARGET, not to the clip
// that instances it: when two clips animate the same node, every clip entry
// carries the last clip's keys over the first clip's duration and the others
// come out empty (measured on the Khronos Fox, whose three clips move the same
// bones: the "Survey" entry holds "Run"'s keys and "Walk"'s groups are empty).
// Such a file cannot be judged by SceneKit between its keys; its rest pose
// still is, and three.js plays each clip correctly.
const SCENEKIT_SHARED_CLIPS =
  "SceneKit's Collada importer binds keys by target, so clips animating the same node replace each other's keys; animation judged by three.js";
function colladaClips(text) {
  const targets = new Map();
  for (const m of text.matchAll(/<animation\b[^>]*\bid="([^"]+)"[^>]*>([\s\S]*?)<\/animation>/g)) {
    targets.set(m[1], [...m[2].matchAll(/<channel\b[^>]*\btarget="([^"/]+)/g)].map((x) => x[1]));
  }
  const clips = [...text.matchAll(/<animation_clip\b([^>]*)>([\s\S]*?)<\/animation_clip>/g)].map((m) => ({
    id: (m[1].match(/\bid="([^"]+)"/) || [])[1],
    name: (m[1].match(/\bname="([^"]+)"/) || [])[1],
    nodes: new Set([...m[2].matchAll(/<instance_animation\b[^>]*\burl="#([^"]+)"/g)].flatMap((x) => targets.get(x[1]) || [])),
  }));
  const seen = new Set();
  let shared = false;
  for (const c of clips) {
    for (const n of c.nodes) if (seen.has(n)) shared = true;
    for (const n of c.nodes) seen.add(n);
  }
  return { clips, shared };
}
let SCENEKIT = null;
async function scenekitBinary() {
  if (SCENEKIT !== null) return SCENEKIT;
  SCENEKIT = false;
  if (process.platform !== 'darwin' || !(await haveTool('swiftc'))) return SCENEKIT;
  const bin = path.join(outDir, '.scenekit-render');
  try {
    await run('swiftc', ['-O', path.join(here, 'scenekit', 'render.swift'), '-o', bin], { timeout: 300000 });
    SCENEKIT = bin;
  } catch (e) {
    console.warn(`SceneKit renderer did not build: ${String(e.stderr || e.message).slice(0, 300)}`);
  }
  return SCENEKIT;
}

async function scenekitRender(file, camera, mode, time, out, clip = null) {
  const bin = await scenekitBinary();
  if (!bin) return null;
  const jobFile = out + '.job.json';
  await fs.writeFile(
    jobFile,
    JSON.stringify({ camera, size: 256, mode, time, clip, vertexColorsAreSRGB: /\.(obj|ply)$/i.test(file) }),
  );
  try {
    const { stdout } = await run(bin, [file, out, jobFile], { timeout: 120000 });
    await fs.rm(jobFile, { force: true });
    return { stats: JSON.parse(stdout.trim().split('\n').pop() || '{}') };
  } catch (e) {
    await fs.rm(jobFile, { force: true });
    return { error: String(e.stderr || e.message).trim().slice(0, 300) };
  }
}

// The root layer's text: the file itself, or a USDZ's first (stored) entry.
async function usdRootText(file) {
  const b = await fs.readFile(file);
  if (!file.endsWith('.usdz')) return b.toString('utf8');
  const size = b.readUInt32LE(18);
  const start = 30 + b.readUInt16LE(26) + b.readUInt16LE(28);
  const head = b.subarray(start, start + Math.min(size, 8));
  return head.toString('latin1') === 'PXR-USDC' ? '' : b.subarray(start, start + size).toString('utf8');
}

function usdCameraLayer(cam, up = [0, 1, 0]) {
  const sub = (a, b) => a.map((v, i) => v - b[i]);
  const norm = (v) => {
    const l = Math.hypot(...v);
    return v.map((x) => x / l);
  };
  const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
  // three.js's lookAt: z = eye - target, x = up x z, y = z x x; USD cameras
  // look down -Z with +Y up, and a row-vector matrix's rows are those axes.
  const z = norm(sub(cam.position, cam.target));
  const x = norm(cross(up, z));
  const y = cross(z, x);
  const f = 50;
  const ap = 2 * f * Math.tan(((cam.fov / 2) * Math.PI) / 180);
  const row = (v, w) => `(${v[0]}, ${v[1]}, ${v[2]}, ${w})`;
  return `#usda 1.0
def Xform "VerifyCameraRoot"
{
    def Camera "VerifyCamera"
    {
        matrix4d xformOp:transform = ( ${row(x, 0)}, ${row(y, 0)}, ${row(z, 0)}, ${row(cam.position, 1)} )
        uniform token[] xformOpOrder = ["xformOp:transform"]
        float focalLength = ${f}
        float horizontalAperture = ${ap}
        float verticalAperture = ${ap}
        float2 clippingRange = (${cam.near}, ${cam.far})
    }
}
`;
}

// Overs that make every UsdPreviewSurface emit exactly its diffuse source and
// reflect nothing: no diffuse, IOR 1 (zero Fresnel reflectance), no normal map.
function usdAlbedoOverrides(text) {
  const tree = {};
  for (const m of text.matchAll(/outputs:surface\.connect = <([^>]+)\.outputs:surface>/g)) {
    const shader = m[1];
    const name = shader.split('/').pop();
    const at = text.indexOf(`def Shader "${name}"`, m.index);
    if (at < 0) continue;
    // The shader's own block: up to the next prim definition.
    const next = text.indexOf('def ', at + 4);
    const body = text.slice(at, next < 0 ? text.length : next);
    // A connection wins over a value authored beside it, as in UsdShade.
    const conn = body.match(/color3f inputs:diffuseColor\.connect = (<[^>]+>)/);
    const value = body.match(/color3f inputs:diffuseColor = (\([^)]*\))/);
    if (!conn && !value) continue;
    let node = tree;
    for (const part of shader.split('/').filter(Boolean)) node = node[part] ??= {};
    node['\0'] = [
      conn ? `color3f inputs:emissiveColor.connect = ${conn[1]}` : `color3f inputs:emissiveColor = ${value[1]}`,
      'color3f inputs:diffuseColor.connect = None',
      'color3f inputs:diffuseColor = (0, 0, 0)',
      'float inputs:ior = 1',
      'float inputs:roughness = 1',
      'float inputs:metallic.connect = None',
      'float inputs:metallic = 0',
      'normal3f inputs:normal.connect = None',
      'float inputs:occlusion.connect = None',
    ];
  }
  const out = [];
  const emit = (node, depth) => {
    for (const [k, v] of Object.entries(node)) {
      if (k === '\0') continue;
      out.push(`${'    '.repeat(depth)}over "${k}"`, `${'    '.repeat(depth)}{`);
      for (const l of v['\0'] || []) out.push(`${'    '.repeat(depth + 1)}${l}`);
      emit(v, depth + 1);
      out.push(`${'    '.repeat(depth)}}`);
    }
  };
  emit(tree, 0);
  return out.join('\n') + '\n';
}

// Render `file` with usdrecord at the default time ('static') and at each
// time; returns { label: pngPath }.
async function usdrecordFrames(file, sessionText, times, tcps, dir, tag, extraArgs) {
  await fs.mkdir(dir, { recursive: true });
  const session = path.join(dir, `${tag}_session.usda`);
  await fs.writeFile(session, sessionText);
  const out = {};
  for (const t of ['static', ...times]) {
    const label = String(t);
    const sub = path.join(dir, `${tag}_${label}`);
    await fs.rm(sub, { recursive: true, force: true });
    await fs.mkdir(sub, { recursive: true });
    const args = ['--sessionLayer', session, '--camera', '/VerifyCameraRoot/VerifyCamera', '--imageWidth', '256', ...extraArgs];
    // `--defaultTime` is a flag (store_true in OpenUSD's framesArgs.py): a
    // value after it is read as the USD file, and argparse then rejects the
    // real output path, so every static render failed.
    if (t === 'static') args.push('--defaultTime');
    else args.push('--frames', String(t * tcps));
    // One retry: usdrecord occasionally fails a frame it renders on the
    // next attempt (seen once in 2,000 frames, while SceneKit was rendering).
    let error = null;
    for (let attempt = 0; attempt < 2 && !out[label]; attempt++) {
      try {
        // A frame range needs a frame placeholder in the output name; the
        // default time must not have one.
        await run('usdrecord', [...args, file, path.join(sub, t === 'static' ? 'f.png' : 'f_###.png')], { timeout: 120000 });
        const made = (await fs.readdir(sub)).filter((f) => f.endsWith('.png'));
        if (made.length) {
          out[label] = path.join(dir, `${tag}_${label}.png`);
          await fs.rename(path.join(sub, made[0]), out[label]);
        } else {
          error = 'usdrecord wrote no image';
        }
      } catch (e) {
        error = String(e.stderr || e.message).trim().split('\n').slice(-2).join(' / ');
      }
    }
    if (!out[label]) {
      out[label] = null;
      process.stderr.write(`usdrecord ${path.relative(matrixDir, file)} @ ${label}: ${error}\n`);
    }
    await fs.rm(sub, { recursive: true, force: true });
  }
  return out;
}

async function validateAssimp(file) {
  try {
    const { stdout } = await run('assimp', ['info', file], { timeout: 60000, maxBuffer: 8 << 20 });
    const grab = (label) => {
      const m = stdout.match(new RegExp(`${label}:\\s+(\\d+)`));
      return m ? Number(m[1]) : null;
    };
    return {
      tool: 'assimp',
      ok: /Meshes:\s+[1-9]/.test(stdout),
      meshes: grab('Meshes'),
      faces: grab('Faces'),
      materials: grab('Materials'),
      textures: grab('Textures \\(embed\\.\\)'),
      animations: grab('Animation Channels') ?? grab('Animations'),
      bones: grab('Bones'),
    };
  } catch (e) {
    // assimp says why it refused a file on stdout, after its progress output.
    const why = String(e.stdout || '').split(/[\r\n%]+/).filter((l) => l.includes('ERROR')).slice(-1);
    return { tool: 'assimp', ok: false, messages: [String(e.stderr || e.message).slice(0, 400), ...why] };
  }
}

async function validateUsd(file) {
  try {
    const { stdout, stderr } = await run('usdchecker', ['--arkit', file], { timeout: 60000 });
    return { tool: 'usdchecker --arkit', ok: true, messages: (stdout + stderr).split('\n').filter(Boolean).slice(-6) };
  } catch (e) {
    return {
      tool: 'usdchecker --arkit',
      ok: false,
      messages: String((e.stdout || '') + (e.stderr || '')).split('\n').filter(Boolean).slice(-12),
    };
  }
}

async function validate(file, format) {
  const out = [];
  if (format === 'glb' || format === 'gltf') out.push(await validateGltf(file));
  if (format.startsWith('usd') && TOOLS.usdchecker) out.push(await validateUsd(file));
  if (TOOLS.assimp && ['fbx', 'dae', '3mf', 'obj', 'ply', 'stl', 'glb', 'gltf'].includes(format)) {
    out.push(await validateAssimp(file));
  }
  return out;
}

// ─── render ─────────────────────────────────────────────────────────────────

function modesFor(caps) {
  const modes = ['geom', 'albedo'];
  if (caps.normals) modes.push('normals');
  if (caps.uv_sets > 0) modes.push('uv');
  return modes;
}

function capsFromKey(key) {
  const m = key.match(/^h(\d)n(\d)u(\d+)v(\d)m(.)t(\d)s(\d)x(\d)a(\d)g(\d)$/);
  return {
    hierarchy: m[1] === '1', normals: m[2] === '1', uv_sets: Number(m[3]), vertex_colors: m[4] === '1',
    materials: m[5], textures: m[6] === '1', skinning: m[7] === '1', morph_targets: m[8] === '1',
    animation: m[9] === '1', multiple_meshes: m[10] === '1',
  };
}

async function saveDataUrl(dataUrl, file) {
  if (!dataUrl) return null;
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, Buffer.from(dataUrl.split(',')[1], 'base64'));
  return path.relative(outDir, file);
}

await fs.rm(outDir, { recursive: true, force: true });
await fs.mkdir(outDir, { recursive: true });

const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
const consoleErrors = [];
page.on('pageerror', (e) => consoleErrors.push(String(e)));
await page.goto(`${base}/page/index.html`);
await page.waitForFunction(() => window.ready === true, null, { timeout: 30000 });

const report = { generated: new Date().toISOString(), base: matrixDir, fixtures: {}, cases: [] };

for (const [name, fx] of Object.entries(manifest.fixtures)) {
  if (onlyFixtures && !onlyFixtures.includes(name)) continue;
  const fxReport = { camera: fx.camera, times: fx.times, expected: {}, native: null };
  report.fixtures[name] = fxReport;
  for (const [key, rel] of Object.entries(fx.expected)) {
    if (!rel) continue;
    // Every mode, and every sample time: cases sharing this picture differ in
    // which modes they compare, and the renderer only animates when the file
    // has a clip.
    const job = {
      key: `${name}|${key}`,
      url: `${base}/m/${rel}`,
      format: 'glb',
      camera: fx.camera,
      modes: ['geom', 'albedo', 'normals', 'uv', 'cover'],
      times: fx.times,
      clipName: fx.animations?.[0],
    };
    const r = await page.evaluate((j) => window.renderExpected(j), job);
    const images = {};
    for (const [mode, byT] of Object.entries(r.images)) {
      images[mode] = {};
      for (const [t, url] of Object.entries(byT)) {
        images[mode][t] = await saveDataUrl(url, path.join(outDir, 'img', name, 'expected', key, `${mode}_${t}.png`));
      }
    }
    fxReport.expected[key] = { path: rel, images, stats: r.stats, validation: await validate(path.join(matrixDir, rel), 'glb') };
    process.stdout.write(`expected ${name} ${key}${r.stats.animated ? ' (animated)' : ''}\n`);
  }
}

// Third-party originals: their own loader's picture against our reading.
for (const [name, fx] of Object.entries(manifest.fixtures)) {
  if (!fx.native || (onlyFixtures && !onlyFixtures.includes(name))) continue;
  const cacheKey = `native:${name}`;
  // A Collada native whose primitives carry their own COLOR input: three.js
  // r183 decodes those through an off-by-one (ColladaComposer.
  // buildGeometryData: `startIndex = array.length - sourceStride - 1`, so of an
  // RGBA colour it converts the previous alpha, R and G, and never B — a file
  // of 0.5 grey loads as [0.214, 0.214, 0.5]; verify/probe.mjs shows it), so
  // its vertex colours cannot be the reference for such a file.
  const nativeText = fx.native.format === 'dae' ? await fs.readFile(path.join(matrixDir, fx.native.path), 'utf8').catch(() => '') : '';
  const primitiveColor = /semantic="COLOR"/.test(nativeText.replace(/<vertices[\s\S]*?<\/vertices>/g, ''));
  const pairModes = primitiveColor ? ['geom', 'albedo', 'albedo_novc'] : fx.native.format === 'fbx' ? ['geom', 'albedo', 'albedo_map'] : ['geom', 'albedo'];
  const r = await page.evaluate(
    ([a, b, k]) => window.renderPair(a, b, k),
    [
      { url: `${base}/m/${fx.native.path}`, format: fx.native.format, files: [], modes: pairModes, times: fx.times || [], clipName: fx.animations?.[0] },
      { url: `${base}/m/${fx.reference}`, format: 'glb', files: [], modes: pairModes, times: fx.times || [], clipName: fx.animations?.[0] },
      cacheKey,
    ],
  );
  const check = { reasons: [] };
  const dir = path.join(outDir, 'img', name, 'native');
  // A second native opinion: the original through SceneKit's own importer,
  // from the camera our reading was framed with.
  let scenekitAlbedo = null;
  let scenekitSilhouette = null;
  if (r.ourCamera && SCENEKIT_FORMATS.has(fx.native.format) && (await scenekitBinary())) {
    check.scenekit = {};
    let undefinedMeshes = false;
    let skCamera = r.ourCamera;
    if (USD_FORMATS[fx.native.format] && TOOLS.usdcat) {
      const text = await run('usdcat', [path.join(matrixDir, fx.native.path)], { timeout: 120000, maxBuffer: 256 << 20 })
        .then((r) => r.stdout)
        .catch(() => '');
      undefinedMeshes = usdMeshUnderOver(text);
      // SceneKit draws a USD stage in its own numbers: it applies neither
      // metersPerUnit nor upAxis (measured on Meshy's USDZ, metersPerUnit
      // 0.3048, drawn 3.28 times too big). Our reader converts both, so the
      // camera framed on our reading goes into the file's space, as it does
      // for usdrecord.
      const meta = (k) => (text.match(new RegExp(`\\n\\s*${k} = ([^\\n]+)`)) || [])[1];
      const zUp = (meta('upAxis') || '"Y"').includes('Z');
      const mpu = Number(meta('metersPerUnit') ?? 0.01);
      const toNative = (p, scale = true) => {
        const q = zUp ? [p[0], -p[2], p[1]] : [...p];
        return scale ? q.map((x) => x / mpu) : q;
      };
      skCamera = { ...r.ourCamera, position: toNative(r.ourCamera.position), target: toNative(r.ourCamera.target), near: r.ourCamera.near / mpu, far: r.ourCamera.far / mpu };
    }
    for (const mode of ['silhouette', 'albedo']) {
      if (SCENEKIT_SKIP[`${fx.native.format}:${mode}`]) {
        check.scenekit[mode] = { skipped: SCENEKIT_SKIP[`${fx.native.format}:${mode}`] };
        continue;
      }
      if (undefinedMeshes) {
        check.scenekit[mode] = { skipped: SCENEKIT_USD_UNDEFINED };
        continue;
      }
      await fs.mkdir(dir, { recursive: true });
      const png = path.join(dir, `scenekit_${mode}_native.png`);
      const sk = await scenekitRender(path.join(matrixDir, fx.native.path), skCamera, mode, null, png);
      if (!sk || sk.error) {
        check.scenekit[mode] = { error: sk ? sk.error : 'renderer unavailable' };
        continue;
      }
      const cmp = await page.evaluate(
        ([url, k, m]) => window.compareImage(url, k, m, 'static'),
        [`${base}/out/${path.relative(outDir, png)}`, cacheKey, mode],
      );
      const item = { image: path.relative(outDir, png), ratio: cmp.ratio, error: cmp.error };
      if (cmp.diff) item.diff = await saveDataUrl(cmp.diff, path.join(dir, `scenekit_${mode}_native_diff.png`));
      check.scenekit[mode] = item;
      const limit = mode === 'silhouette' ? 0.01 : 0.02;
      if (cmp.error || cmp.ratio > limit) {
        check.reasons.push(`SceneKit ${mode}: ${cmp.error || (cmp.ratio * 100).toFixed(2) + '% differ from the native file'}`);
      }
      if (mode === 'albedo' && !cmp.error) scenekitAlbedo = cmp.ratio <= limit;
      if (mode === 'silhouette' && !cmp.error) scenekitSilhouette = cmp.ratio <= limit;
    }
  }
  for (const [mode, url] of Object.entries(r.ours || {})) {
    check[mode] = { ours: await saveDataUrl(url, path.join(dir, `${mode}_ours.png`)) };
  }
  if (r.error) {
    // three.js r183's ColladaLoader throws on every morph controller
    // (ColladaParser.parseController stores a morph controller with no
    // `skin`; ColladaComposer.buildNode then reads `controller.skin.joints`,
    // and a skin over a morph fails in buildController reading `sources`).
    // For such a file the native verdict is SceneKit's, which implements
    // Collada morphs — and only when SceneKit agrees with our reading.
    if (r.nativeFailed && fx.native.format === 'dae' && scenekitAlbedo === true && scenekitSilhouette === true) {
      check.threeLimitation = `three.js cannot load this file (${r.error.slice(0, 160)}); SceneKit's native rendering matches ours`;
      process.stdout.write(`note native ${name}: three.js failed to load it; SceneKit's native rendering matches ours\n`);
    } else {
      check.reasons.push(r.error);
    }
  }
  for (const [mode, res] of Object.entries(r.results || {})) {
    check[mode] = {
      ratio: res.ratio,
      native: await saveDataUrl(res.native, path.join(dir, `${mode}_native.png`)),
      ours: await saveDataUrl(res.ours, path.join(dir, `${mode}_ours.png`)),
      diff: await saveDataUrl(res.diff, path.join(dir, `${mode}_diff.png`)),
    };
    if (res.ratio > 0.05) {
      // three.js's ColladaLoader draws some valid Collada wrongly, and for
      // those the native albedo verdict is SceneKit's (above) — never a looser
      // threshold. Evidence (three r183, examples/jsm/loaders/collada):
      // - ColladaParser.parseGeometryPrimitive names a TEXCOORD input by its
      //   `set` alone ('TEXCOORD' for 0, 'TEXCOORD1' for 1) and nothing reads
      //   <bind_vertex_input>, so a texture bound to set 1 (SceneKit writes its
      //   only set as set="1") is sampled through `uv1` and drawn as one texel;
      // - ColladaComposer.buildGeometryType concatenates every primitive's
      //   COLOR into one array, so a <mesh> mixing primitives with and without
      //   COLOR shifts the colours of every later vertex (research ref.dae:
      //   its red triangle draws black).
      if (mode === 'albedo' && fx.native.format === 'dae' && scenekitAlbedo === true) {
        check[mode].threeLimitation =
          'three.js ColladaLoader misreads this file (bind_vertex_input / mixed COLOR primitives); SceneKit agrees with our reading';
        process.stdout.write(`note native ${name}: three.js albedo ${(res.ratio * 100).toFixed(1)}% off; SceneKit's native rendering matches ours\n`);
        continue;
      }
      // No second renderer for it (SceneKit refuses some files outright):
      // excuse the vertex colours only, and only when everything else —
      // materials, textures, UVs — matches with them left out.
      const novc = r.results?.albedo_novc;
      if (mode === 'albedo' && primitiveColor && scenekitAlbedo !== false && novc && novc.ratio <= 0.05) {
        check[mode].threeLimitation = `three.js decodes primitive-level COLOR through an off-by-one; without vertex colours the native rendering matches ours (${(novc.ratio * 100).toFixed(2)}%)`;
        process.stdout.write(`note native ${name}: three.js albedo ${(res.ratio * 100).toFixed(1)}% off, all of it in its primitive-level vertex colours; ${(novc.ratio * 100).toFixed(2)}% without them\n`);
        continue;
      }
      // three.js r183's FBXLoader multiplies a classic material's colour into
      // the map beside it (parseParameters sets `color` from DiffuseColor and
      // `map` together, and MeshPhongMaterial multiplies them). The SDK,
      // Maya and Blender let the map replace the colour, and so does our
      // reader: Blender writes its socket default 0.8 there, so Meshy's FBX
      // draws 40 % darker in three.js than its own GLB of the same model.
      // Excused only when, with every mapped colour left to its map, the
      // original matches our reading.
      const mapAlone = r.results?.albedo_map;
      if (mode === 'albedo' && fx.native.format === 'fbx' && mapAlone && mapAlone.ratio <= 0.05) {
        check[mode].threeLimitation = `three.js FBXLoader multiplies DiffuseColor into the map beside it, where the SDK, Maya and Blender use the map alone; with mapped colours left to their maps the original matches our reading (${(mapAlone.ratio * 100).toFixed(2)}%)`;
        process.stdout.write(`note native ${name}: three.js albedo ${(res.ratio * 100).toFixed(1)}% off; ${(mapAlone.ratio * 100).toFixed(2)}% with mapped colours left to their maps\n`);
        continue;
      }
      if (mode === 'albedo_novc' || mode === 'albedo_map') continue;
      check.reasons.push(`${mode}: ${(res.ratio * 100).toFixed(1)}% differ from the native rendering`);
    }
  }
  // Animated frames of the native file against our reading of its clip.
  for (const [t, res] of Object.entries(r.animResults || {})) {
    check.anim = check.anim || {};
    check.anim[t] = {
      ratio: res.ratio,
      native: await saveDataUrl(res.native, path.join(dir, `geom_${t}_native.png`)),
      ours: await saveDataUrl(res.ours, path.join(dir, `geom_${t}_ours.png`)),
      diff: await saveDataUrl(res.diff, path.join(dir, `geom_${t}_diff.png`)),
    };
    if (res.ratio > 0.05) check.reasons.push(`geom@${t}: ${(res.ratio * 100).toFixed(1)}% differ from the native rendering`);
  }
  if (USD_FORMATS[fx.native.format]) {
    // three.js r183's USDLoader cannot judge a USD original: beyond the
    // defects listed in page/render.js (sRGB colours, displayColor, USDA
    // rotations, no BlendShapes), USDComposer._resolveReference matches only
    // `@asset@<prim>` (USDComposer.js:768), so an internal reference — how
    // Apple's importer instances a mesh — renders nothing, and
    // _buildGeometryWithSubsets drops every face no GeomSubset covers. Its
    // pictures stay in the report; usdrecord, Pixar's renderer, decides.
    for (const mode of ['geom', 'albedo']) {
      if (check[mode]) check[mode].skipped = 'three.js USDLoader is not a judge of USD originals; usdrecord decides';
    }
    if (check.anim) for (const a of Object.values(check.anim)) a.skipped = 'three.js USDLoader is not a judge of USD originals; usdrecord decides';
    check.reasons = check.reasons.filter((x) => !/^(albedo|geom)[:@]|^native usd/.test(x));
    await usdNativeCheck(name, fx, check);
  }
  check.nativeStats = r.nativeStats;
  check.ourStats = r.ourStats;
  report.fixtures[name].nativeCheck = check;
  process.stdout.write(`${check.reasons.length ? 'DIFF' : 'ok  '} native ${name}${check.reasons.length ? '  ' + check.reasons.join('; ') : ''}\n`);
}

let failures = 0;
for (const c of manifest.cases) {
  if (onlyFixtures && !onlyFixtures.includes(c.fixture)) continue;
  if (onlyFormats && !(onlyFormats.includes(c.to) && (c.from === 'scene' || onlyFormats.includes(c.from)))) continue;
  const fx = manifest.fixtures[c.fixture];
  const entry = { ...c, verdict: 'pass', reasons: [], results: {}, validation: [] };
  report.cases.push(entry);
  if (c.error && c.expected_error) {
    // Refused, and rightly: the expected scene cannot be written either
    // (Apple's skinned USDZ has no area at rest, so there is no 3MF of it).
    entry.verdict = 'refused';
    entry.reasons.push(`refused: ${c.error} (the expected scene cannot be written either: ${c.expected_error})`);
    process.stdout.write(`ref  ${c.fixture} ${c.from} -> ${c.to}  ${c.error}\n`);
    continue;
  }
  if (c.error) {
    entry.verdict = 'fail';
    entry.reasons.push(`conversion error: ${c.error}`);
    failures++;
    continue;
  }
  const primary = path.join(matrixDir, c.primary);
  entry.validation = await validate(primary, c.to);
  const expectedValidation = report.fixtures[c.fixture]?.expected?.[c.expected]?.validation || [];
  for (const v of entry.validation) {
    if (v.ok) continue;
    // `assimp info` post-processes with degenerate-triangle removal, so a
    // scene whose expected pose has no area at all reads as "No meshes
    // remaining" — and then the expected GLB itself is refused the same way,
    // and the refusal is the content's, not the conversion's. The one input
    // that does this is tp_skinned_apple_usdz: Apple's glTF importer turned
    // the hip's cubic scale keys into a scale of 0 at t = 0, so the leg
    // collapses to a point at rest; usdrecord draws it empty at the default
    // time and at t = 0 alike (report img/<fixture>/native/usdrecord_cover_static.png).
    const collapsed = (x) => x.tool === 'assimp' && !x.ok && /No meshes remaining/.test((x.messages || []).join(' '));
    if (collapsed(v) && expectedValidation.some(collapsed)) {
      v.skipped = 'the expected pose has no area (every triangle degenerate), which assimp removes; the expected GLB is refused alike';
      continue;
    }
    entry.reasons.push(`${v.tool} rejected the file${v.messages ? ': ' + v.messages.slice(0, 3).join(' / ') : ''}`);
  }
  const job = {
    url: `${base}/m/${c.primary}`,
    format: c.to,
    files: c.files,
    camera: fx.camera,
    modes: modesFor(c.caps),
    times: c.caps.animation ? fx.times : [],
    expectedKey: `${c.fixture}|${c.expected}`,
    clipName: fx.animations?.[0],
  };
  const r = await page.evaluate((j) => window.renderCase(j), job);
  if (r.error) {
    entry.reasons.push(`${c.to} loader failed: ${r.error}`);
  } else {
    entry.stats = r.stats;
    if (r.stats.texturesLoaded === false) entry.reasons.push('a texture the file references never loaded');
    for (const [mode, byT] of Object.entries(r.results)) {
      entry.results[mode] = {};
      for (const [t, res] of Object.entries(byT)) {
        const dir = path.join(outDir, 'img', c.fixture, `${c.from}__${c.to}`);
        const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, maxDelta: res.maxDelta };
        if (res.skipped) {
          item.skipped = res.skipped;
        } else if (res.error) {
          item.error = res.error;
          entry.reasons.push(`${mode}@${t}: ${res.error}`);
        } else {
          item.image = await saveDataUrl(res.image, path.join(dir, `${mode}_${t}.png`));
          if (res.ratio > THRESHOLD[mode]) {
            item.diff = await saveDataUrl(res.diff, path.join(dir, `${mode}_${t}_diff.png`));
            entry.reasons.push(`${mode}@${t}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
          }
        }
        entry.results[mode][t] = item;
      }
    }
  }
  // Second opinion from SceneKit, for the formats it reads.
  if (SCENEKIT_FORMATS.has(c.to) && (await scenekitBinary())) {
    entry.scenekit = {};
    const dir = path.join(outDir, 'img', c.fixture, `${c.from}__${c.to}`);
    const times = [null, ...(c.caps.animation ? fx.times : [])];
    let clip = null;
    let animated = true;
    const info = await page.evaluate((k) => window.expectedStats(k), `${c.fixture}|${c.expected}`);
    const restSkip = USD_FORMATS[c.to] && info?.morphDefaults ? SCENEKIT_USD_MORPH_DEFAULTS : null;
    const albedoSkip = c.to === 'obj' && (await mtlTintsAMap(path.join(matrixDir, c.dir), c.files)) ? SCENEKIT_OBJ_KD : null;
    if (c.to === 'dae' && times.length > 1) {
      const { clips, shared } = colladaClips(await fs.readFile(primary, 'utf8'));
      const want = fx.animations?.[0];
      clip = (clips.find((k) => k.name === want || k.id === want) || clips[0])?.id ?? null;
      if (clips.length > 1 && shared) animated = false;
    }
    for (const mode of ['silhouette', 'albedo']) {
      if (SCENEKIT_SKIP[`${c.to}:${mode}`]) {
        entry.scenekit[mode] = { skipped: SCENEKIT_SKIP[`${c.to}:${mode}`] };
        continue;
      }
      if (mode === 'albedo' && albedoSkip) {
        entry.scenekit[mode] = { skipped: albedoSkip };
        continue;
      }
      entry.scenekit[mode] = {};
      for (const t of times) {
        const key = t === null ? 'static' : String(t);
        if (t !== null && !animated) {
          entry.scenekit[mode][key] = { skipped: SCENEKIT_SHARED_CLIPS };
          continue;
        }
        if (t === null && restSkip) {
          entry.scenekit[mode][key] = { skipped: restSkip };
          continue;
        }
        const png = path.join(dir, `scenekit_${mode}_${key}.png`);
        await fs.mkdir(dir, { recursive: true });
        const r = await scenekitRender(primary, fx.camera, mode, t, png, clip);
        if (!r || r.error) {
          entry.scenekit[mode][key] = { error: r ? r.error : 'renderer unavailable' };
          if (r && r.error) entry.reasons.push(`SceneKit could not render the ${c.to}: ${r.error}`);
          continue;
        }
        const cmp = await page.evaluate(
          ([url, expectedKey, mode, key]) => window.compareImage(url, expectedKey, mode, key),
          [`${base}/out/${path.relative(outDir, png)}`, `${c.fixture}|${c.expected}`, mode, key],
        );
        const item = { image: path.relative(outDir, png), ratio: cmp.ratio, stats: r.stats };
        const limit = mode === 'silhouette' ? 0.01 : 0.02;
        if (cmp.error) {
          item.error = cmp.error;
        } else if (cmp.ratio > limit) {
          item.diff = await saveDataUrl(cmp.diff, path.join(dir, `scenekit_${mode}_${key}_diff.png`));
          entry.reasons.push(`SceneKit ${mode}@${key}: ${(cmp.ratio * 100).toFixed(2)}% differ`);
        }
        entry.scenekit[mode][key] = item;
      }
    }
  }
  if (USD_FORMATS[c.to] && !r.error) {
    await usdOracle(c, fx, entry, r.stats);
  }
  if (entry.reasons.length) {
    entry.verdict = 'fail';
    failures++;
  }
  const flag = entry.verdict === 'pass' ? 'ok  ' : 'FAIL';
  process.stdout.write(`${flag} ${c.fixture} ${c.from} -> ${c.to}${entry.reasons.length ? '  ' + entry.reasons.slice(0, 2).join('; ') : ''}\n`);
}

// A USD original rendered by usdrecord against our reading rendered by
// three.js, through one camera: framed on our reading, then carried into the
// original's own space (up axis and units), which is what our reader converts.
async function usdNativeCheck(name, fx, check) {
  if (!TOOLS.usdrecord || !TOOLS.usdcat) {
    check.reasons.push('usdrecord/usdcat are not installed; the USD original is unjudged');
    return;
  }
  const nativeFile = path.join(matrixDir, fx.native.path);
  const work = path.join(matrixDir, name, 'native_usdrecord');
  const rel = path.relative(matrixDir, work).split(path.sep).join('/');
  let text;
  try {
    text = (await run('usdcat', ['--flatten', nativeFile], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  } catch (e) {
    check.reasons.push(`usdcat cannot flatten the original: ${String(e.stderr || e.message).slice(0, 200)}`);
    return;
  }
  const meta = (k) => (text.match(new RegExp(`\\n\\s*${k} = ([^\\n]+)`)) || [])[1];
  const zUp = (meta('upAxis') || '"Y"').includes('Z');
  const mpu = Number(meta('metersPerUnit') ?? 0.01);
  const tcps = Number(meta('timeCodesPerSecond') ?? meta('framesPerSecond') ?? 24);
  const cam = await page.evaluate(([u]) => window.frameOf(u, 'glb'), [`${base}/m/${fx.reference}`]);
  // Our reader maps the original's points p to Rx(-90°)·p·mpu for Z-up; the
  // camera goes the other way.
  const toNative = (p, scale = true) => {
    const q = zUp ? [p[0], -p[2], p[1]] : [...p];
    return scale ? q.map((v) => v / mpu) : q;
  };
  const nativeCam = {
    fov: cam.fov,
    position: toNative(cam.position),
    target: toNative(cam.target),
    near: cam.near / mpu,
    far: cam.far / mpu,
  };
  const up = toNative([0, 1, 0], false);
  const times = fx.times || [];
  await page.evaluate((j) => window.renderExpected(j), {
    key: `native|${name}`,
    url: `${base}/m/${fx.reference}`,
    format: 'glb',
    camera: cam,
    modes: ['cover', 'albedo'],
    times,
    clipName: fx.animations?.[0],
  });
  const camLayer = usdCameraLayer(nativeCam, up);
  const passes = [
    { mode: 'cover', kind: 'silhouette', session: camLayer, args: [] },
    { mode: 'albedo', kind: 'albedo', session: camLayer + usdAlbedoOverrides(text), args: ['--disableCameraLight'] },
  ];
  const dir = path.join(outDir, 'img', name, 'native');
  for (const pass of passes) {
    const frames = await usdrecordFrames(nativeFile, pass.session, times, tcps, work, pass.mode, pass.args);
    for (const [label, file] of Object.entries(frames)) {
      const key = `usdrecord:${pass.mode}@${label}`;
      if (!file) {
        check.reasons.push(`${key}: usdrecord failed on the original`);
        continue;
      }
      const res = await page.evaluate((j) => window.compareExternal(j), {
        url: `${base}/m/${rel}/${path.basename(file)}`,
        expectedKey: `native|${name}`,
        mode: pass.mode,
        t: label,
        kind: pass.kind,
      });
      check[key] = {
        ratio: res.ratio,
        native: await saveDataUrl(res.image, path.join(dir, `usdrecord_${pass.mode}_${label}.png`)),
        diff: res.diff ? await saveDataUrl(res.diff, path.join(dir, `usdrecord_${pass.mode}_${label}_diff.png`)) : null,
      };
      if (res.error) check.reasons.push(`${key}: ${res.error}`);
      else if (res.ratio > USD_THRESHOLD[pass.mode]) check.reasons.push(`${key}: ${(res.ratio * 100).toFixed(1)}% differ from usdrecord's rendering of the original`);
    }
  }
}

async function usdOracle(c, fx, entry, stats) {
  const primary = path.join(matrixDir, c.primary);
  const times = c.caps.animation ? fx.times : [];
  const work = path.join(matrixDir, c.dir, 'usdrecord');
  const rel = path.relative(matrixDir, work).split(path.sep).join('/');
  const imgDir = path.join(outDir, 'img', c.fixture, `${c.from}__${c.to}`);
  const text = await usdRootText(primary);
  const tcps = Number((text.match(/timeCodesPerSecond = ([0-9.eE+-]+)/) || [])[1] || 24);
  const camera = usdCameraLayer(fx.camera);
  if (!TOOLS.usdrecord) {
    entry.reasons.push('usdrecord is not installed; USD is only half verified');
    return;
  }
  const passes = [
    { mode: 'cover', kind: 'silhouette', session: camera, args: [] },
    { mode: 'albedo', kind: 'albedo', session: camera + usdAlbedoOverrides(text), args: ['--disableCameraLight'] },
  ];
  for (const pass of passes) {
    const frames = await usdrecordFrames(primary, pass.session, times, tcps, work, pass.mode, pass.args);
    const key = `usdrecord:${pass.mode}`;
    entry.results[key] = {};
    for (const [label, file] of Object.entries(frames)) {
      if (!file) {
        entry.reasons.push(`${key}@${label}: usdrecord failed`);
        continue;
      }
      const res = await page.evaluate((j) => window.compareExternal(j), {
        url: `${base}/m/${rel}/${path.basename(file)}`,
        expectedKey: `${c.fixture}|${c.expected}`,
        mode: pass.mode,
        t: label,
        kind: pass.kind,
      });
      const item = { ratio: res.ratio, differ: res.differ, covered: res.covered };
      if (res.error) {
        item.error = res.error;
        entry.reasons.push(`${key}@${label}: ${res.error}`);
      } else {
        item.image = await saveDataUrl(res.image, path.join(imgDir, `${key.replace(':', '_')}_${label}.png`));
        if (res.ratio > USD_THRESHOLD[pass.mode]) {
          item.diff = await saveDataUrl(res.diff, path.join(imgDir, `${key.replace(':', '_')}_${label}_diff.png`));
          entry.reasons.push(`${key}@${label}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
        }
      }
      entry.results[key][label] = item;
    }
  }
  // Skeletal animation through three.js's (correct) crate reader.
  const info = await page.evaluate((k) => window.expectedStats?.(k), `${c.fixture}|${c.expected}`);
  if (TOOLS.usdcat && info && info.skinned && times.length) {
    const copy = path.join(work, 'copy.usdc');
    try {
      await run('usdcat', [primary, '-o', copy], { timeout: 120000 });
      const r = await page.evaluate((j) => window.renderCase(j), {
        url: `${base}/m/${rel}/copy.usdc`,
        format: 'usdc',
        files: ['copy.usdc'],
        camera: fx.camera,
        modes: ['geom'],
        times,
        expectedKey: `${c.fixture}|${c.expected}`,
        clipName: fx.animations?.[0],
      });
      entry.results['crate:geom'] = {};
      if (r.error) {
        entry.reasons.push(`crate:geom: three.js could not load the usdcat copy: ${r.error}`);
      } else {
        for (const [t, res] of Object.entries(r.results.geom || {})) {
          const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, skipped: res.skipped };
          if (!res.skipped && !res.error) {
            item.image = await saveDataUrl(res.image, path.join(imgDir, `crate_geom_${t}.png`));
            if (res.ratio > THRESHOLD.geom) {
              item.diff = await saveDataUrl(res.diff, path.join(imgDir, `crate_geom_${t}_diff.png`));
              entry.reasons.push(`crate:geom@${t}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
            }
          }
          entry.results['crate:geom'][t] = item;
        }
      }
    } catch (e) {
      entry.reasons.push(`crate:geom: usdcat failed: ${String(e.stderr || e.message).slice(0, 200)}`);
    }
  }
}

await browser.close();
server.close();
report.consoleErrors = consoleErrors;
report.summary = {
  cases: report.cases.length,
  passed: report.cases.filter((c) => c.verdict === 'pass').length,
  refused: report.cases.filter((c) => c.verdict === 'refused').length,
  failed: failures,
  // Which independent checks this run had: a missing one is a weaker run,
  // not a passing one, and says so here.
  tools: { ...TOOLS, scenekit: !!SCENEKIT },
};
await fs.writeFile(path.join(outDir, 'report.json'), JSON.stringify(report, null, 1));
await fs.writeFile(path.join(outDir, 'report.html'), renderHtml(report));
const refusedNote = report.summary.refused ? `, ${report.summary.refused} refused as the expected scene is` : '';
const missing = Object.entries(report.summary.tools).filter(([, ok]) => !ok).map(([t]) => t);
if (missing.length) console.log(`\nnot installed, so not checked: ${missing.join(', ')}`);
console.log(`\n${report.summary.passed}/${report.summary.cases} cases pass${refusedNote} -> ${path.join(outDir, 'report.html')}`);
process.exit(failures ? 1 : 0);

// ─── report page ────────────────────────────────────────────────────────────

function esc(s) {
  return String(s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[c]);
}

function renderHtml(rep) {
  const parts = [];
  parts.push(`<!doctype html><meta charset="utf-8"><title>Mesh conversion matrix</title>
<style>
body{font:13px system-ui,sans-serif;margin:16px;background:#fafafa;color:#222}
table{border-collapse:collapse;margin:8px 0 24px}td,th{border:1px solid #ddd;padding:4px;vertical-align:top;text-align:center}
img{width:128px;height:128px;image-rendering:pixelated;background:#fff;display:block}
.fail{background:#fde8e8}.pass{background:#e9f7ec}.refused{background:#f3efe2}.na{background:#eee;color:#888}
small{display:block;max-width:150px;word-break:break-word;color:#555}
</style><h1>Mesh conversion matrix</h1>
<p>${rep.summary.passed}/${rep.summary.cases} cases pass${rep.summary.refused ? `, ${rep.summary.refused} refused as the expected scene is` : ''}. Rows: source container (scene = written directly from the fixture). Columns: target container. Each cell is the target file rendered by three.js's own loader for that format.</p>`);
  const formats = [...new Set(rep.cases.map((c) => c.to))];
  const froms = ['scene', ...formats];
  for (const [name, fx] of Object.entries(rep.fixtures)) {
    for (const mode of ['albedo', 'geom', 'normals', 'uv', 'usdrecord:albedo', 'usdrecord:cover', 'crate:geom']) {
      const t = fx.times && fx.times.length ? String(fx.times[2] ?? fx.times[0]) : 'static';
      parts.push(`<h2>${esc(name)} — ${mode}${t !== 'static' ? ` @ t=${Number(t).toFixed(2)}s` : ''}</h2><table><tr><th>from \\ to</th>`);
      for (const f of formats) parts.push(`<th>${esc(f)}</th>`);
      parts.push('</tr>');
      for (const from of froms) {
        parts.push(`<tr><th>${esc(from)}</th>`);
        for (const to of formats) {
          const c = rep.cases.find((x) => x.fixture === name && x.from === from && x.to === to);
          if (!c) {
            parts.push('<td class="na">—</td>');
            continue;
          }
          const res = c.results?.[mode]?.[t] ?? c.results?.[mode]?.static;
          const cls = c.verdict === 'pass' || c.verdict === 'refused' ? c.verdict : 'fail';
          if (!res || !res.image) {
            parts.push(`<td class="${cls}"><small>${esc(c.reasons[0] || 'n/a')}</small></td>`);
            continue;
          }
          parts.push(`<td class="${cls}"><img src="${esc(res.image)}"><small>${(res.ratio * 100).toFixed(2)}% differ</small>${res.diff ? `<img src="${esc(res.diff)}">` : ''}</td>`);
        }
        parts.push('</tr>');
      }
      parts.push('</table>');
    }
  }
  parts.push('<h2>Failures and refusals</h2><ul>');
  for (const c of rep.cases.filter((x) => x.verdict !== 'pass')) {
    parts.push(`<li><b>${esc(c.fixture)} ${esc(c.from)} → ${esc(c.to)}</b>: ${c.reasons.map(esc).join('; ')}</li>`);
  }
  parts.push('</ul>');
  return parts.join('\n');
}
