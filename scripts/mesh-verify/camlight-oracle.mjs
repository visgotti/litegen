#!/usr/bin/env node
// The numeric oracle for lights and cameras (mesh-gaps item 5, spec "Render
// harness"): assimp's reading of a file, compared with the scene's own values
// within 1e-4.
//
//   node camlight-oracle.mjs <matrix_dir> [--fixtures a,b] [--json <file>] [--work <dir>]
//   node camlight-oracle.mjs --file <file.fbx|.dae|.glb|.gltf> --expected <expected.glb> [--json <file>]
//
// The spec reads "assimp's glTF conversion" of every FBX and Collada output.
// assimp 6.0's glTF2 exporter writes no camera and no light, so the oracle
// reads its assjson export instead (item 5's Ruling R11; the controller's
// probe in the item's ledger): `assimp export <file> <out>.assjson -fassjson`
// carries the same aiCamera and aiLight values a glTF export would. The
// scene's values come from the case's expected GLB, read here from its glTF
// JSON (no three.js, no litegen code): what a correct conversion must hold.
// verify.mjs runs it on every FBX, Collada and glTF case and on every
// expected GLB (its calibration: assimp's glTF importer reads cameras and
// lights too), reads every USD case's cameras from its flattened text
// (usdCameras), and counts both in its report's `summary.camlightOracle`.
// Each camera's frame (eye, forward and up) is compared at rest and, on an
// animated case, at each sample time: the scene's from its first animation
// (sceneCameras), assimp's from its first (assimpWorld), a USD stage's from
// its xformOps and their time samples (usdCameras). Only these numbers judge
// a camera the pictures do not draw through, and a near or far plane that
// cuts no geometry (the camera pictures show depth only where a plane cuts
// something). A node with no name is paired by its depth-first rank
// (page/node-rank.js, nameCandidates).
//
// Where assimp's convention differs from the scene's, the comparison is
// adjusted and the citation recorded (assimp v6.0.5; the files are
// code/AssetLib/FBX/FBXConverter.cpp, code/AssetLib/Collada/ColladaLoader.cpp
// and ColladaParser.cpp, code/AssetLib/glTF2/glTF2Importer.cpp and
// code/AssetLib/Assjson/json_exporter.cpp): see ASSIMP below. A USD stage's
// cameras are read from its own flattened text instead (usdCameras,
// USD_TEXT). Item 5's rulings, R1–R32, are listed with their costs in the
// "Rulings" section of docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md.

import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { depthFirstRanks, unnamedKey } from './page/node-rank.js';

const run = promisify(execFile);

// ─── tolerance ──────────────────────────────────────────────────────────────

// The spec's 1e-4, scale-aware: assjson prints every number at the stream's
// default precision, 6 significant digits (json_exporter.cpp L171-L174 and
// L228, `stream << f`, no precision set), so a value above 1 (a far plane of
// 64, a colour × intensity of 40) is held to 1e-4 of itself. Directions are
// compared by the angle between them, within 1e-4 rad.
export const TOLERANCE = 1e-4;
export const within = (got, want, tol = TOLERANCE) =>
  Number.isFinite(got) && Number.isFinite(want) && Math.abs(got - want) <= tol * Math.max(1, Math.abs(want));

// ─── 4 × 4 matrices, row-major ──────────────────────────────────────────────
//
// assimp's aiMatrix4x4 order (json_exporter.cpp L268-L276 writes row by row,
// the translation in the fourth column), and the order the glTF reader below
// converts its column-major matrices to.

export function mat4Mul(a, b) {
  const out = new Array(16).fill(0);
  for (let r = 0; r < 4; r++) {
    for (let c = 0; c < 4; c++) {
      let s = 0;
      for (let k = 0; k < 4; k++) s += a[r * 4 + k] * b[k * 4 + c];
      out[r * 4 + c] = s;
    }
  }
  return out;
}
export const mat4Point = (m, p) => [0, 1, 2].map((r) => m[r * 4] * p[0] + m[r * 4 + 1] * p[1] + m[r * 4 + 2] * p[2] + m[r * 4 + 3]);
export const mat4Dir = (m, v) => [0, 1, 2].map((r) => m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2]);

// T · R · S, from a translation, a unit quaternion [x, y, z, w] and a scale.
export function trsMatrix(t, q, s) {
  const [x, y, z, w] = q;
  const r = [
    1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w),
    2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w),
    2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y),
  ];
  return [
    r[0] * s[0], r[1] * s[1], r[2] * s[2], t[0],
    r[3] * s[0], r[4] * s[1], r[5] * s[2], t[1],
    r[6] * s[0], r[7] * s[1], r[8] * s[2], t[2],
    0, 0, 0, 1,
  ];
}

// Spherical interpolation of [x, y, z, w] quaternions, the shorter way round,
// as assimp's own aiQuaternion::Interpolate does.
export function quatSlerp(a, b, u) {
  let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
  const s = d < 0 ? -1 : 1;
  d *= s;
  if (d > 0.9995) {
    const q = a.map((v, i) => v + (s * b[i] - v) * u);
    const l = Math.hypot(...q);
    return q.map((v) => v / l);
  }
  const th = Math.acos(Math.min(1, d));
  const ka = Math.sin((1 - u) * th) / Math.sin(th);
  const kb = (s * Math.sin(u * th)) / Math.sin(th);
  return a.map((v, i) => ka * v + kb * b[i]);
}

const dot = (a, b) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a, b) => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (v) => {
  const l = Math.hypot(...v);
  return l > 0 ? v.map((x) => x / l) : v;
};
// The angle between two directions, accurate near 0 (atan2 of the sine and
// cosine, not acos).
const angleBetween = (a, b) => Math.atan2(Math.hypot(...cross(norm(a), norm(b))), dot(norm(a), norm(b)));

// IEC 61966-2-1's encoding, which litegen's FBX writer applies to a light's
// Color by default (FbxColorSpace::Srgb, item 5's Ruling R14).
export const srgbEncode = (c) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055);
export const srgbDecode = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));

// ─── what assimp does differently, per importer, with its source lines ─────

const AS = 'assimp v6.0.5';
export const ASSIMP = {
  fbx: {
    // FBXConverter.cpp L501-L504: "FBX fov is full-view degrees. We want
    // half-view radians." (FieldOfView is litegen's horizontal angle beside
    // the horizontal gate fit, fbx_write.rs camera_attribute.)
    hfov: 'half',
    hfovCite: `${AS} FBXConverter.cpp L501-L504 stores half the horizontal FieldOfView`,
    // L472-L508 reads AspectWidth/AspectHeight, FieldOfView (else FilmWidth
    // and FocalLength), NearPlane and FarPlane, and no CameraProjectionType.
    projection: null,
    projectionSkip: `${AS} FBXConverter.cpp L472-L508 reads no CameraProjectionType (nor OrthoZoom), so an orthographic camera reads as a perspective one`,
    magSkip: `${AS} FBXConverter.cpp L472-L508 reads no OrthoZoom, and the assjson exporter writes no aiCamera::mOrthographicWidth (json_exporter.cpp L684-L709)`,
    orthoAspect: true,
    // L427-L428 copies InnerAngle and OuterAngle, in radians; litegen writes
    // full cones (fbx_write.rs light_attribute, Blender's convention), so the
    // scene's half-angles are half of assimp's.
    cones: 'full',
    conesCite: `${AS} FBXConverter.cpp L425-L428 takes InnerAngle and OuterAngle as they stand; litegen writes full cones, so each scene half-angle is half of assimp's`,
    // L401-L407 multiplies Intensity/100 into Color, read as it stands;
    // litegen writes Color sRGB-encoded by default (FbxColorSpace::Srgb,
    // Ruling R14), and the matrix uses the default (mesh_matrix.rs).
    color: 'srgb',
    colorCite: `${AS} FBXConverter.cpp L401-L407 multiplies Intensity/100 into Color; litegen's default FbxColorSpace::Srgb writes Color sRGB-encoded (Ruling R14), so the scene's side is srgb(colour) × intensity`,
    // L445-L469: DecayType 2 (quadratic) gives constant 0, linear 0,
    // quadratic 2/DecayStart².
    falloffCite: `${AS} FBXConverter.cpp L457-L461 reads DecayType 2 as constant 0, linear 0, quadratic 2/DecayStart²`,
  },
  dae: {
    // ColladaLoader.cpp L422-L436: xfov, or 2·atan(aspect·tan(yfov/2)), in
    // degrees, then radians: the full horizontal angle.
    hfov: 'full',
    hfovCite: `${AS} ColladaLoader.cpp L422-L436 stores the full horizontal fov`,
    projection: null,
    projectionSkip: `${AS} ColladaLoader.cpp L400-L403 reads an orthographic camera as a perspective one ("Orthographic cameras are not supported"), and ColladaParser.cpp L396-L401 reads xmag and ymag into the fov fields`,
    magSkip: `${AS} ColladaParser.cpp L398-L401 reads xmag and ymag as xfov and yfov, and the assjson exporter writes no aiCamera::mOrthographicWidth (json_exporter.cpp L684-L709)`,
    orthoAspect: false,
    orthoAspectSkip: `${AS} ColladaLoader.cpp L425-L428 makes tan(xmag°)/tan(ymag°) an orthographic camera's aspect, from xmag and ymag read as fovs (ColladaParser.cpp L398-L401)`,
    // L357-L372: falloff_angle becomes the inner cone and the outer is
    // guessed from falloff_exponent (colladaInnerCone).
    cones: 'collada',
    conesCite: `${AS} ColladaLoader.cpp L357-L372 makes falloff_angle (the full cone, twice the outer half-angle, Ruling R23) its inner cone and guesses its outer cone from falloff_exponent; the oracle halves the first, inverts the guess and applies dae.rs's documented falloff_exponent rule`,
    color: 'linear',
    colorCite: `${AS} ColladaLoader.cpp L347-L355 reads <color> × <intensity> (1 when absent); litegen's <color> is linear colour × intensity (Ruling R17)`,
    falloffCite: `${AS} ColladaParser.cpp L357-L362 reads the three attenuations as written`,
  },
  glb: {
    // glTF2Importer.cpp L924-L926: 2·atan(tan(yfov/2)·aspect), the full angle.
    hfov: 'full',
    hfovCite: `${AS} glTF2Importer.cpp L924-L926 stores the full horizontal fov`,
    // L929-L937: an orthographic camera has a horizontal fov of 0 and
    // xmag/ymag as its aspect.
    projection: 'zero-fov',
    magSkip: `${AS} glTF2Importer.cpp L933 keeps xmag as aiCamera::mOrthographicWidth, which the assjson exporter does not write (json_exporter.cpp L684-L709)`,
    orthoAspect: true,
    // L998-L1000 copies glTF's half-angles.
    cones: 'half',
    conesCite: `${AS} glTF2Importer.cpp L998-L1000 copies innerConeAngle and outerConeAngle`,
    color: 'linear',
    colorCite: `${AS} glTF2Importer.cpp L975-L978 multiplies the intensity into the colour`,
    falloffCite: `${AS} glTF2Importer.cpp L993-L995 writes the inverse square as constant 0, linear 0, quadratic 1`,
  },
};
ASSIMP.gltf = ASSIMP.glb;
// A USD stage's cameras are read from its flattened text (usdCameras), not
// by assimp: what UsdGeomCamera holds (OpenUSD v25.05, the installed Apple
// USD Tools' line; gf/camera.cpp is the same at v26.11.1).
const USD_SCHEMA = 'OpenUSD v25.05';
export const USD_TEXT = {
  projection: 'read',
  mag: true,
  orthoAspect: true,
  // A camera's frame: its prim's xformOps composed with its ancestors'
  // (usdWorld), at the stage's default values or at a time code.
  xformCite: `${USD_SCHEMA} pxr/usd/usdGeom/xformable.cpp L1001-L1028 (the ops multiplied in reverse: in column-vector terms local = op[0] · op[1] · …) and L645-L651 (!resetXformStack! drops the ops before it, and the parent's transform), xformOp.cpp L521-L548 (a transform op, a row-vector matrix), L550-L594 (the single-axis ops, rotations in degrees), L596-L668 (translate, scale, and rotateXYZ etc., X first for rotateXYZ), L670-L687 (orient, a quaternion written real part first) and L697-L705 (an op with no value is the identity); time samples interpolate linearly, quaternions by slerp the shorter way round (usd/interpolators.h L140-L163, gf/quatf.cpp L159-L189), and the first or last sample holds outside them (sdf/data.cpp L337-L352); a time code is seconds × timeCodesPerSecond, which falls back to framesPerSecond and then 24 (sdf/layer.cpp L1726-L1744); the stage is turned Y-up and to metres as litegen's reader turns it (usd_read.rs L5-L9: upAxis Z by −90° about X; metersPerUnit's fallback 0.01, usdGeom/schema.usda L2287-L2294)`,
  cite: `${USD_SCHEMA} pxr/usd/usdGeom/schema.usda L2352-L2373 (projection, apertures and focal length in tenths of a scene unit, clippingRange in scene units, and their defaults), pxr/base/gf/camera.cpp L274-L297 (aspect = horizontalAperture / verticalAperture, yfov = 2·atan(verticalAperture / (2·focalLength))) and L311-L326 (an orthographic half-width of aperture × 0.1 / 2)`,
};

// The prims of a flattened USD stage (`usdcat --flatten`), with their paths,
// in document order: each header line's indent places it under the nearest
// header above it that is indented less. `body`: the prim's own lines, up to
// the next prim header.
export function usdPrims(text) {
  const open = [];
  const out = [];
  const lines = text.split('\n');
  lines.forEach((line, i) => {
    const m = line.match(/^(\s*)(def|over|class)\s+(?:(\w+)\s+)?"([^"]+)"/);
    if (!m) return;
    while (open.length && open[open.length - 1].indent >= m[1].length) open.pop();
    const prim = { indent: m[1].length, specifier: m[2], type: m[3] ?? '', name: m[4], path: `${open.length ? open[open.length - 1].path : ''}/${m[4]}`, line: i };
    open.push(prim);
    out.push(prim);
  });
  out.forEach((p, k) => {
    p.body = lines.slice(p.line, out[k + 1]?.line ?? lines.length).join('\n');
  });
  return out;
}

// ─── names ──────────────────────────────────────────────────────────────────

// Every name a reader may give the scene's node `key`. A node with no name
// is `node ${rank}` to the scene (sceneCameras, sceneLights) and to the page
// (page/render.js lightNodeName), `rank` being the node's place in a
// depth-first walk of the default scene (page/node-rank.js, which is also
// the order litegen's reader numbers its nodes in, glb.rs `nodes`,
// L2794-L2864), not its place in the glTF's `nodes` array; item 5's Ruling
// R25, amended by followups 2026-11-08 item A, pairs it by that rank:
// litegen's writers name it after its IR index, which is that rank,
// `node{i}` in FBX (fbx_write.rs, the fallback it hands `Names::unique`) and as a Collada id (dae.rs L5129,
// which writes no name: assimp names a node after its id,
// ColladaLoader.cpp L1801-L1822, and render.swift names a nameless SceneKit
// node by its id) and `node_{i}` in USD (usd_write.rs, its `node_{n}` fallbacks); assimp's glTF
// importer names it by its index in the file it reads, `nodes[i]`
// (glTF2Importer.cpp L1106-L1108, glTF2Asset.inl L506): in a file litegen
// wrote, its IR order, the rank (assimp reads outputs alone, never the
// expected file, whose own array order may differ). A named node keeps its
// name in glTF and FBX (fbx_write.rs `Names::unique`, which
// replaces control characters alone), and takes each other writer's
// sanitised form of it (colladaName, colladaId, usdPrimName), and SceneKit's
// reading of the Collada name (sceneKitColladaName). Not foreseen: a writer's
// de-duplicating suffix (`-2`, `_1`), so of two names that sanitise alike the
// first pairs, and the index fallback of a name that sanitises to `_` alone.
export function nameCandidates(key) {
  const m = /^node (\d+)$/.exec(key);
  if (m) return [key, `node${m[1]}`, `node_${m[1]}`, `nodes[${m[1]}]`];
  return [...new Set([key, colladaName(key), colladaId(key), usdPrimName(key), sceneKitColladaName(key)])];
}

// A Collada `name` attribute, as dae.rs `ncname` writes it (L4898-L4919): an
// XML NCName, every character an NCName cannot hold made `_`, and a `_` in
// front of a name that cannot start one. SceneKit names a Collada node by it.
const NC_START = 'A-Z_a-zÀ-ÖØ-öø-˿Ͱ-ͽͿ-῿‌-‍⁰-↏Ⰰ-⿯、-퟿豈-﷏ﷰ-�\u{10000}-\u{EFFFF}';
const NC_START_RE = new RegExp(`^[${NC_START}]`, 'u');
const NC_REST_RE = new RegExp(`^[${NC_START}\\-.0-9·̀-ͯ‿-⁀]$`, 'u');
export function colladaName(name) {
  const out = Array.from(name, (c) => (NC_REST_RE.test(c) ? c : '_')).join('');
  return NC_START_RE.test(out) ? out : `_${out}`;
}
// SceneKit's name for a Collada node: its `name` attribute (colladaName)
// with each `.` made `-` (measured 2026-11-06, macOS 26.6.2: names a.b.c,
// Lámpara.1, m·n.o, x-y_z and p:q read as a-b-c, Lámpara-1, m·n-o, x-y_z and
// p:q; Khronos LightsPunctualLamp's Point.002_Orientation as
// Point-002_Orientation).
export const sceneKitColladaName = (name) => colladaName(name).replaceAll('.', '-');

// A Collada id, as dae.rs `Ids::make` writes it (L4869-L4895): ASCII letters,
// digits, `_` and `-` kept, every other character `_`, at most 64 of them,
// and a `_` in front unless it starts with a letter or `_`. assimp names a
// Collada node after its id (ColladaLoader.cpp L1801-L1822).
export function colladaId(name) {
  const out = Array.from(name, (c) => (/^[A-Za-z0-9_-]$/.test(c) ? c : '_')).slice(0, 64).join('');
  return /^[A-Za-z_]/.test(out) ? out : `_${out}`;
}
// A USD prim name, as usd_write.rs `identifier` writes it: ASCII
// letters, digits and `_` kept, every other character `_`, at most 96 of
// them, and a `_` in front of a leading digit.
export function usdPrimName(name) {
  const out = Array.from(name, (c) => (/^[A-Za-z0-9_]$/.test(c) ? c : '_')).slice(0, 96).join('');
  return /^[0-9]/.test(out) ? `_${out}` : out;
}

// ─── USD transforms ─────────────────────────────────────────────────────────

const I4 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

// The layer's up axis, units and time codes (its header, before any prim).
function usdStage(text) {
  const end = text.search(/^\s*(?:def|over|class)\s/m);
  const head = end < 0 ? text : text.slice(0, end);
  const num = (key) => {
    const m = head.match(new RegExp(`^\\s*${key} = ([-+0-9.eE]+)\\s*$`, 'm'));
    return m ? Number(m[1]) : undefined;
  };
  const up = (head.match(/^\s*upAxis = "(\w+)"/m) || [])[1] ?? 'Y';
  const authored = num('metersPerUnit');
  const mpu = authored === undefined ? 0.01 : authored > 0 && Number.isFinite(authored) ? authored : 1;
  const turn = up === 'Z' ? [1, 0, 0, 0, 0, 0, 1, 0, 0, -1, 0, 0, 0, 0, 0, 1] : I4;
  return {
    mpu,
    tcps: num('timeCodesPerSecond') ?? num('framesPerSecond') ?? 24,
    error: up === 'Y' || up === 'Z' ? null : `the stage's upAxis is ${up}, neither Y nor Z`,
    // Stage space to the scene's: Y up, in metres.
    toScene: mat4Mul([mpu, 0, 0, 0, 0, mpu, 0, 0, 0, 0, mpu, 0, 0, 0, 0, 1], turn),
  };
}

// A prim's own lines, after its metadata (a `customData` dictionary is not
// read) and before its first child.
function usdOwnText(p) {
  const lines = p.body.split('\n');
  const open = lines.findIndex((l, i) => i > 0 && l.trim() === '{' && l.length - l.trimStart().length === p.indent);
  return (open < 0 ? lines.slice(1) : lines.slice(open + 1)).join('\n');
}
const USD_NUMBER = /[-+]?(?:inf|nan|(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)/g;
const usdNumbers = (s) => (s.trim() === 'None' ? null : (s.match(USD_NUMBER) || []).map((x) => (x === 'inf' || x === '+inf' ? Infinity : x === '-inf' ? -Infinity : Number(x))));

// Each op of a prim's xformOpOrder, its default value and its time samples.
function usdOps(own) {
  const order = own.match(/^\s*uniform token\[\] xformOpOrder = \[([^\]]*)\]/m);
  let ops = [];
  let reset = false;
  for (const [, token] of (order?.[1] ?? '').matchAll(/"([^"]+)"/g)) {
    if (token === '!resetXformStack!') {
      reset = true;
      ops = [];
      continue;
    }
    const invert = token.startsWith('!invert!');
    const name = invert ? token.slice('!invert!'.length) : token;
    const type = (name.match(/^xformOp:(\w+)/) || [])[1];
    const esc = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const dflt = own.match(new RegExp(`^\\s*(?:custom\\s+)?(\\w+)\\s+${esc}\\s*=\\s*(.+?)\\s*$`, 'm'));
    const dict = own.match(new RegExp(`^\\s*(?:custom\\s+)?(\\w+)\\s+${esc}\\.timeSamples\\s*=\\s*\\{([^}]*)\\}`, 'm'));
    const samples = [];
    for (const line of (dict?.[2] ?? '').split('\n')) {
      const m = line.match(/^\s*([-+0-9.eE]+)\s*:\s*(.+?),?\s*$/);
      if (m) samples.push([Number(m[1]), usdNumbers(m[2])]);
    }
    samples.sort((a, b) => a[0] - b[0]);
    const valueType = (dflt || dict)?.[1] ?? '';
    ops.push({ name, type, invert, value: dflt ? usdNumbers(dflt[2]) : null, samples, quat: valueType.startsWith('quat') });
  }
  return { ops, reset };
}

// An op's value at time code `code` (null: the default value).
function usdOpValue(op, code) {
  const ks = op.samples;
  if (code === null || !ks.length) return op.value;
  if (code <= ks[0][0]) return ks[0][1];
  if (code >= ks[ks.length - 1][0]) return ks[ks.length - 1][1];
  let i = 0;
  while (ks[i + 1][0] <= code) i++;
  const [t0, a] = ks[i];
  const [t1, b] = ks[i + 1];
  if (!a || !b) return a;
  const u = (code - t0) / (t1 - t0);
  if (op.quat) {
    // Written real part first; quatSlerp takes [x, y, z, w].
    const [x, y, z, w] = quatSlerp([a[1], a[2], a[3], a[0]], [b[1], b[2], b[3], b[0]], u);
    return [w, x, y, z];
  }
  return a.map((v, k) => v + (b[k] - v) * u);
}

// An op's matrix (row-major, column vectors, as mat4Mul's).
const AXES = { X: 0, Y: 1, Z: 2 };
function axisRotation(axis, degrees) {
  const t = (degrees * Math.PI) / 180;
  const c = Math.cos(t);
  const s = Math.sin(t);
  if (axis === 'X') return [1, 0, 0, 0, 0, c, -s, 0, 0, s, c, 0, 0, 0, 0, 1];
  if (axis === 'Y') return [c, 0, s, 0, 0, 1, 0, 0, -s, 0, c, 0, 0, 0, 0, 1];
  return [c, -s, 0, 0, s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
}
const translation = ([x, y, z]) => [1, 0, 0, x, 0, 1, 0, y, 0, 0, 1, z, 0, 0, 0, 1];
const scaling = ([x, y, z]) => [x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, 0, 0, 0, 0, 1];
const OP_SIZE = { translate: 3, scale: 3, orient: 4, transform: 16 };
function usdOpMatrix(type, v) {
  const want = OP_SIZE[type] ?? (/^(?:translate|scale|rotate)[XYZ]$/.test(type) ? 1 : /^rotate[XYZ]{3}$/.test(type) ? 3 : 0);
  if (!want) throw new Error(`xformOp type ${type} is not one UsdGeomXformOp defines`);
  if (v.length !== want) throw new Error(`xformOp:${type} has ${v.length} value(s), not ${want}`);
  if (type === 'translate') return translation(v);
  if (type === 'scale') return scaling(v);
  if (type === 'orient') return trsMatrix([0, 0, 0], norm4([v[1], v[2], v[3], v[0]]), [1, 1, 1]);
  // USD's row-vector matrix, transposed.
  if (type === 'transform') return [0, 1, 2, 3].flatMap((r) => [0, 1, 2, 3].map((c) => v[c * 4 + r]));
  const axis = type.slice(-1);
  if (/^translate[XYZ]$/.test(type)) return translation([0, 1, 2].map((k) => (k === AXES[axis] ? v[0] : 0)));
  if (/^scale[XYZ]$/.test(type)) return scaling([0, 1, 2].map((k) => (k === AXES[axis] ? v[0] : 1)));
  if (/^rotate[XYZ]$/.test(type)) return axisRotation(axis, v[0]);
  // rotateXYZ and its kin: the angles in X, Y, Z order, applied in the
  // name's order.
  let m = I4;
  for (const a of type.slice('rotate'.length)) m = mat4Mul(axisRotation(a, v[AXES[a]]), m);
  return m;
}
const norm4 = (q) => {
  const l = Math.hypot(...q);
  return l > 0 ? q.map((x) => x / l) : q;
};
// The inverse of an affine matrix.
function affineInverse(m) {
  const [a, b, c, d, e, f, g, h, i] = [m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]];
  const det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
  const r = [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det, (f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det, (d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det];
  const t = [m[3], m[7], m[11]];
  const ti = [0, 1, 2].map((k) => -(r[k * 3] * t[0] + r[k * 3 + 1] * t[1] + r[k * 3 + 2] * t[2]));
  return [r[0], r[1], r[2], ti[0], r[3], r[4], r[5], ti[1], r[6], r[7], r[8], ti[2], 0, 0, 0, 1];
}

// Every prim's world matrix in stage space at time code `code` (null: the
// default values), by path, or the error that keeps it from one (USD_TEXT
// .xformCite). A prim no UsdGeomXformable schema covers adds no transform.
const XFORMLESS = new Set(['', 'Scope', 'Material', 'Shader', 'NodeGraph', 'GeomSubset', 'SkelAnimation', 'BlendShape']);
function usdWorld(prims, code) {
  const out = new Map();
  for (const p of prims) {
    const parentPath = p.path.slice(0, p.path.lastIndexOf('/'));
    const parent = parentPath ? out.get(parentPath) : { m: I4 };
    if (!parent || parent.error) {
      out.set(p.path, { error: parent?.error ?? `the prim ${parentPath} above ${p.path} was not read` });
      continue;
    }
    if (XFORMLESS.has(p.type)) {
      out.set(p.path, parent);
      continue;
    }
    try {
      const { ops, reset } = usdOps(usdOwnText(p));
      let local = I4;
      for (const op of ops) {
        const v = usdOpValue(op, code);
        if (!v) continue;
        const m = usdOpMatrix(op.type, v);
        local = mat4Mul(local, op.invert ? affineInverse(m) : m);
      }
      out.set(p.path, { m: reset ? local : mat4Mul(parent.m, local) });
    } catch (e) {
      out.set(p.path, { error: `${p.path}: ${e.message}` });
    }
  }
  return out;
}

// The Camera prims of a flattened USD stage, as UsdGeomCamera defines them
// (USD_TEXT.cite): each prim's own attribute lines, after its metadata (a
// `customData` dictionary is not read), each missing one at the schema's
// default. Apertures and focal length are in tenths of a scene unit, so a
// perspective yfov is 2·atan(verticalAperture / (2·focalLength)) and an
// orthographic xmag is horizontalAperture / 20; the aspect is
// horizontalAperture / verticalAperture; clippingRange is (near, far). Its
// frame is its prim's world matrix (usdWorld) at `seconds` (null: the
// default values, the rest pose), a camera looking along its −Z with +Y up
// (UsdGeomCamera), turned into the scene's space: Y up, lengths in metres.
export function usdCameras(text, seconds = null) {
  const num = '([-+0-9.eE]+|inf|-inf|nan)';
  const stage = usdStage(text);
  const prims = usdPrims(text);
  const world = usdWorld(prims, seconds === null ? null : seconds * stage.tcps);
  return prims
    .filter((p) => p.type === 'Camera')
    .map((p) => {
      const own = usdOwnText(p);
      const value = (s) => (s === 'inf' ? Infinity : s === '-inf' ? -Infinity : Number(s));
      const attr = (re, fallback) => {
        const m = own.match(re);
        return m ? m.slice(1).map(value) : fallback;
      };
      const projection = (own.match(/^\s*(?:uniform\s+)?token projection = "(\w+)"/m) || [])[1] ?? 'perspective';
      const [focal] = attr(new RegExp(`^\\s*float focalLength = ${num}\\s*$`, 'm'), [50]);
      const [h] = attr(new RegExp(`^\\s*float horizontalAperture = ${num}\\s*$`, 'm'), [20.955]);
      const [v] = attr(new RegExp(`^\\s*float verticalAperture = ${num}\\s*$`, 'm'), [15.2908]);
      const [znear, zfar] = attr(new RegExp(`^\\s*float2 clippingRange = \\(${num}, ${num}\\)\\s*$`, 'm'), [1, 1000000]);
      const w = world.get(p.path);
      if (stage.error || w.error) return { name: p.name, path: p.path, error: stage.error ?? w.error };
      const m = mat4Mul(stage.toScene, w.m);
      const u = stage.mpu;
      const rec = {
        name: p.name,
        path: p.path,
        eye: mat4Point(m, [0, 0, 0]),
        forward: norm(mat4Dir(m, [0, 0, -1])),
        up: norm(mat4Dir(m, [0, 1, 0])),
        projection,
        aspect: h / v,
        znear: znear * u,
        zfar: zfar * u,
      };
      if (projection === 'orthographic') Object.assign(rec, { xmag: (h / 20) * u, ymag: (v / 20) * u });
      else rec.yfov = 2 * Math.atan(v / (2 * focal));
      return rec;
    });
}
// assjson writes no aiCamera::mPosition (json_exporter.cpp L684-L709), and
// every importer here leaves it at 0 (FBXConverter.cpp L482; the default
// aiCamera, camera.h L184-L191, for Collada and glTF), so a camera's eye is
// its node's origin.

// ─── the assjson document ───────────────────────────────────────────────────

// assimp writes NaN and ±Infinity as the strings "NaN", "Infinity" and
// "-Infinity" (Flag_WriteSpecialFloats, on by default: json_exporter.cpp
// L202-L226, L804-L806). They are numbers again here, except as names.
const SPECIAL = { NaN: NaN, Infinity: Infinity, '-Infinity': -Infinity };
export const parseAssjson = (text) =>
  JSON.parse(text, (key, value) => (key !== 'name' && typeof value === 'string' && Object.hasOwn(SPECIAL, value) ? SPECIAL[value] : value));

const vecAt = (keys, ticks) => {
  if (ticks <= keys[0][0]) return keys[0][1];
  const last = keys[keys.length - 1];
  if (ticks >= last[0]) return last[1];
  let i = 0;
  while (keys[i + 1][0] <= ticks) i++;
  const [t0, a] = keys[i];
  const [t1, b] = keys[i + 1];
  const u = (ticks - t0) / (t1 - t0);
  return a.map((v, k) => v + (b[k] - v) * u);
};
// assjson quaternions are [w, x, y, z] (json_exporter.cpp L251-L258).
const toXyzw = ([w, x, y, z]) => [x, y, z, w];
const quatAt = (keys, ticks) => {
  if (ticks <= keys[0][0]) return toXyzw(keys[0][1]);
  const last = keys[keys.length - 1];
  if (ticks >= last[0]) return toXyzw(last[1]);
  let i = 0;
  while (keys[i + 1][0] <= ticks) i++;
  const [t0, a] = keys[i];
  const [t1, b] = keys[i + 1];
  return quatSlerp(toXyzw(a), toXyzw(b), (ticks - t0) / (t1 - t0));
};

// Each node's world matrix, by name (the first node of a name, as assimp
// binds a camera or light to the node of its name), at rest or with the
// file's first animation at `seconds`. A channel replaces its node's matrix
// with T·R·S of its keys at that time (assimp's aiNodeAnim); keys are in
// ticks, `tickspersecond` of them per second, and the first or last key
// holds outside them. A component with no keys is the identity's: assimp's
// glTF importer writes no key for one the node does not write
// (glTF2Importer.cpp L1330-L1480, CreateNodeAnim), as a node without a
// scale; its FBX and Collada importers write all three.
export function assimpWorld(aj, seconds = null) {
  const anim = seconds === null ? null : (aj.animations || [])[0];
  const ticks = anim ? seconds * (anim.tickspersecond > 0 ? anim.tickspersecond : 25) : 0;
  const channels = new Map((anim?.channels || []).map((c) => [c.name, c]));
  const out = new Map();
  const walk = (n, parent) => {
    let local = n.transformation;
    const ch = channels.get(n.name);
    if (ch && (ch.positionkeys?.length || ch.rotationkeys?.length || ch.scalingkeys?.length)) {
      local = trsMatrix(
        ch.positionkeys?.length ? vecAt(ch.positionkeys, ticks) : [0, 0, 0],
        ch.rotationkeys?.length ? quatAt(ch.rotationkeys, ticks) : [0, 0, 0, 1],
        ch.scalingkeys?.length ? vecAt(ch.scalingkeys, ticks) : [1, 1, 1],
      );
    }
    const world = parent ? mat4Mul(parent, local) : local;
    if (!out.has(n.name)) out.set(n.name, world);
    for (const c of n.children || []) walk(c, world);
  };
  if (aj.rootnode) walk(aj.rootnode, null);
  return out;
}

// assimp's cameras as the scene describes one: eye, forward and up in world
// space, the vertical fov, aspect and clip planes, with the importer's fov
// convention undone (ASSIMP[format].hfov).
export function assimpCameras(aj, format, seconds = null) {
  const conv = ASSIMP[format];
  const world = assimpWorld(aj, seconds);
  return (aj.cameras || []).map((c) => {
    const m = world.get(c.name);
    if (!m) return { name: c.name, error: `assimp hung camera ${c.name} on no node of that name` };
    const ortho = conv.projection === 'zero-fov' && c.horizontalfov === 0;
    const full = conv.hfov === 'half' ? 2 * c.horizontalfov : c.horizontalfov;
    return {
      name: c.name,
      eye: mat4Point(m, [0, 0, 0]),
      forward: norm(mat4Dir(m, c.lookat)),
      up: norm(mat4Dir(m, c.up)),
      projection: conv.projection === 'zero-fov' ? (ortho ? 'orthographic' : 'perspective') : 'unread',
      yfov: ortho ? undefined : 2 * Math.atan(Math.tan(full / 2) / (c.aspect > 0 ? c.aspect : 1)),
      aspect: c.aspect,
      znear: c.clipplanenear,
      zfar: c.clipplanefar,
    };
  });
}

// aiLightSourceType (light.h): 1 directional, 2 point, 3 spot.
const KINDS = { 1: 'directional', 2: 'point', 3: 'spot' };

// The inner cone, a half-angle in radians, of a Collada spot as assimp read
// it. assimp makes falloff_angle its inner cone (ColladaLoader.cpp L359), so
// that is the full cone, twice the scene's outer half-angle (Ruling R23), and
// guesses its outer one as acos(0.1^(1/e)) plus it, e the falloff_exponent
// (L367-L372), so e = ln 0.1 / ln cos(outerA − innerA). dae.rs's documented
// rule (`inner_cone`, dae.rs L987-L1007) then gives the inner cone from the
// outer half-angle: the spec's beam cosᵉ is at half where cos θ = 2^(−1/e),
// and glTF's falloff ((cos θ − cos outer)/(cos inner − cos outer))² is at
// half there when cos inner = cos outer + √2 (cos θ − cos outer); a beam too
// weak to halve inside the cone is a hard edge, the outer cone itself.
// `innerA` is assimp's inner cone (the scene's full cone), `outerA` its guess.
export function colladaInnerCone(innerA, outerA) {
  const outer = innerA / 2;
  const e = Math.log(0.1) / Math.log(Math.cos(outerA - innerA));
  const co = Math.cos(Math.min(outer, Math.PI / 2));
  const half = e > 0 ? Math.pow(0.5, 1 / e) : 0;
  const ci = co + Math.SQRT2 * (half - co);
  if (!(ci > co)) return outer;
  if (ci >= 1) return 0;
  return Math.acos(ci);
}

// assimp's lights as the scene describes one: kind, world position and
// direction, colour × intensity as assimp holds it, the cones as scene
// half-angles (ASSIMP[format].cones undone), and the attenuation.
export function assimpLights(aj, format, seconds = null) {
  const conv = ASSIMP[format];
  const world = assimpWorld(aj, seconds);
  return (aj.lights || []).map((l) => {
    const m = world.get(l.name);
    if (!m) return { name: l.name, error: `assimp hung light ${l.name} on no node of that name` };
    const kind = KINDS[l.type] ?? `aiLightSourceType ${l.type}`;
    const out = {
      name: l.name,
      kind,
      position: mat4Point(m, l.position || [0, 0, 0]),
      product: l.diffusecolor,
      attenuation: [l.attenuationconstant, l.attenuationlinear, l.attenuationquadratic],
    };
    if (kind !== 'point' && l.direction) out.direction = norm(mat4Dir(m, l.direction));
    if (kind === 'spot') {
      if (conv.cones === 'full') {
        out.inner = l.angleinnercone / 2;
        out.outer = l.angleoutercone / 2;
      } else if (conv.cones === 'half') {
        out.inner = l.angleinnercone;
        out.outer = l.angleoutercone;
      } else {
        out.outer = l.angleinnercone / 2;
        out.inner = colladaInnerCone(l.angleinnercone, l.angleoutercone);
      }
    }
    return out;
  });
}

// ─── the expected scene, from its glTF JSON ─────────────────────────────────

// A GLB's JSON chunk, or a .gltf's JSON.
export function readGltfJson(bytes) {
  return readGlb(bytes).json;
}
// A GLB's JSON and BIN chunks (glTF 2.0 §4.4: the BIN chunk, type 0x004E4942,
// follows the JSON one), or a .gltf's JSON and no BIN.
export function readGlb(bytes) {
  const b = Buffer.from(bytes);
  if (!(b.length >= 20 && b.readUInt32LE(0) === 0x46546c67)) return { json: JSON.parse(b.toString('utf8')), bin: null };
  const len = b.readUInt32LE(12);
  const json = JSON.parse(b.subarray(20, 20 + len).toString('utf8'));
  const at = 20 + len;
  const bin = b.length >= at + 8 && b.readUInt32LE(at + 4) === 0x004e4942 ? b.subarray(at + 8, at + 8 + b.readUInt32LE(at)) : null;
  return { json, bin };
}

// An accessor's elements, each an array of numbers (glTF 2.0 §3.6: a
// normalized integer is scaled to 0..1 or -1..1; a sparse accessor is not
// read here).
const COMPONENTS = { SCALAR: 1, VEC2: 2, VEC3: 3, VEC4: 4, MAT4: 16 };
const COMPONENT = {
  5120: [1, (v, o) => v.getInt8(o), 127],
  5121: [1, (v, o) => v.getUint8(o), 255],
  5122: [2, (v, o) => v.getInt16(o, true), 32767],
  5123: [2, (v, o) => v.getUint16(o, true), 65535],
  5125: [4, (v, o) => v.getUint32(o, true), 0],
  5126: [4, (v, o) => v.getFloat32(o, true), 0],
};
function accessorRows(json, bin, index) {
  const a = json.accessors?.[index];
  if (!a || a.sparse || a.bufferView === undefined) throw new Error(`accessor ${index} is missing, sparse or has no buffer view`);
  const view = json.bufferViews[a.bufferView];
  if ((view.buffer ?? 0) !== 0 || !bin) throw new Error(`accessor ${index} is not in the GLB's binary chunk`);
  const [size, get, max] = COMPONENT[a.componentType];
  const n = COMPONENTS[a.type];
  const stride = view.byteStride || n * size;
  const base = (view.byteOffset || 0) + (a.byteOffset || 0);
  const dv = new DataView(bin.buffer, bin.byteOffset, bin.byteLength);
  const rows = [];
  for (let k = 0; k < a.count; k++) {
    const row = [];
    for (let c = 0; c < n; c++) {
      const x = get(dv, base + k * stride + c * size);
      row.push(a.normalized && max ? Math.max(x / max, -1) : x);
    }
    rows.push(row);
  }
  return rows;
}

// A sampler's value at `t` seconds (glTF 2.0 §3.11, Appendix C): LINEAR
// (rotations by slerp, the shorter way round), STEP, or CUBICSPLINE (a
// Hermite curve; each key is in-tangent, value, out-tangent, the tangents
// per second); the first or last key holds outside them.
function samplerAt(times, values, interpolation, t, rotation) {
  const cubic = interpolation === 'CUBICSPLINE';
  const value = (k) => (cubic ? values[3 * k + 1] : values[k]);
  const last = times.length - 1;
  if (t <= times[0]) return value(0);
  if (t >= times[last]) return value(last);
  let i = 0;
  while (times[i + 1] <= t) i++;
  const dt = times[i + 1] - times[i];
  const u = (t - times[i]) / dt;
  if (interpolation === 'STEP') return value(i);
  if (cubic) {
    const [p0, m0, p1, m1] = [values[3 * i + 1], values[3 * i + 2], values[3 * i + 4], values[3 * i + 3]];
    const u2 = u * u;
    const u3 = u2 * u;
    const v = p0.map((_, k) => (2 * u3 - 3 * u2 + 1) * p0[k] + (u3 - 2 * u2 + u) * dt * m0[k] + (-2 * u3 + 3 * u2) * p1[k] + (u3 - u2) * dt * m1[k]);
    return rotation ? norm4(v) : v;
  }
  return rotation ? quatSlerp(values[i], values[i + 1], u) : values[i].map((x, k) => x + (values[i + 1][k] - x) * u);
}

// Each animated node's translation, rotation and scale at `seconds` in the
// scene's first animation (assimp's FBX judge and its first animation;
// verify.mjs plays the fixture's first clip), by node index.
function gltfPose(json, bin, seconds) {
  const pose = new Map();
  const anim = (json.animations || [])[0];
  if (!anim) return pose;
  for (const ch of anim.channels || []) {
    const { node, path: prop } = ch.target || {};
    if (node === undefined || !['translation', 'rotation', 'scale'].includes(prop)) continue;
    const s = anim.samplers[ch.sampler];
    const times = accessorRows(json, bin, s.input).map((r) => r[0]);
    const v = samplerAt(times, accessorRows(json, bin, s.output), s.interpolation || 'LINEAR', seconds, prop === 'rotation');
    if (!pose.has(node)) pose.set(node, {});
    pose.get(node)[prop] = v;
  }
  return pose;
}

// Each node's world matrix (row-major) by node index, at rest or at
// `seconds` in the first animation (gltfPose; `bin` holds its keys), for the
// nodes of the default scene; undefined for a node the scene does not reach.
export function gltfWorld(json, seconds = null, bin = null) {
  const nodes = json.nodes || [];
  const out = new Array(nodes.length);
  const pose = seconds === null ? new Map() : gltfPose(json, bin, seconds);
  const local = (n, i) => {
    if (n.matrix) return [0, 1, 2, 3].flatMap((r) => [0, 1, 2, 3].map((c) => n.matrix[c * 4 + r]));
    const a = pose.get(i) || {};
    return trsMatrix(a.translation || n.translation || [0, 0, 0], a.rotation || n.rotation || [0, 0, 0, 1], a.scale || n.scale || [1, 1, 1]);
  };
  const walk = (i, parent) => {
    if (out[i] || !nodes[i]) return;
    out[i] = parent ? mat4Mul(parent, local(nodes[i], i)) : local(nodes[i], i);
    for (const c of nodes[i].children || []) walk(c, out[i]);
  };
  const scene = (json.scenes || [])[json.scene ?? 0];
  for (const r of scene?.nodes || []) walk(r, null);
  return out;
}

// The scene's cameras, one per node that places one (glTF cameras look along
// −Z with +Y up), in node order, at rest or at `seconds` (gltfWorld). A node
// with no name is `node ${rank}`, its depth-first rank (page/node-rank.js,
// nameCandidates).
export function sceneCameras(json, seconds = null, bin = null) {
  const world = gltfWorld(json, seconds, bin);
  const ranks = depthFirstRanks(json);
  const out = [];
  (json.nodes || []).forEach((n, i) => {
    if (n.camera === undefined || !world[i] || !json.cameras?.[n.camera]) return;
    const c = json.cameras[n.camera];
    const m = world[i];
    const rec = { name: n.name || unnamedKey(ranks, i), eye: mat4Point(m, [0, 0, 0]), forward: norm(mat4Dir(m, [0, 0, -1])), up: norm(mat4Dir(m, [0, 1, 0])) };
    if (c.type === 'orthographic') {
      const o = c.orthographic;
      Object.assign(rec, { projection: 'orthographic', xmag: o.xmag, ymag: o.ymag, aspect: o.xmag / o.ymag, znear: o.znear, zfar: o.zfar });
    } else {
      const p = c.perspective;
      Object.assign(rec, { projection: 'perspective', yfov: p.yfov, aspect: p.aspectRatio, znear: p.znear, zfar: p.zfar });
    }
    out.push(rec);
  });
  return out;
}

// The scene's lights, one per node that places one, with KHR_lights_punctual's
// defaults filled in (colour white, intensity 1, cones 0 and π/4); lights
// shine along −Z.
export function sceneLights(json) {
  const world = gltfWorld(json);
  const defs = json.extensions?.KHR_lights_punctual?.lights || [];
  const ranks = depthFirstRanks(json);
  const out = [];
  (json.nodes || []).forEach((n, i) => {
    const li = n.extensions?.KHR_lights_punctual?.light;
    if (li === undefined || !world[i] || !defs[li]) return;
    const d = defs[li];
    const m = world[i];
    const rec = { name: n.name || unnamedKey(ranks, i), kind: d.type, position: mat4Point(m, [0, 0, 0]), color: d.color ?? [1, 1, 1], intensity: d.intensity ?? 1 };
    if (d.range !== undefined) rec.range = d.range;
    if (d.type !== 'point') rec.direction = norm(mat4Dir(m, [0, 0, -1]));
    if (d.type === 'spot') {
      rec.inner = d.spot?.innerConeAngle ?? 0;
      rec.outer = d.spot?.outerConeAngle ?? Math.PI / 4;
    }
    out.push(rec);
  });
  return out;
}

// ─── the comparison ─────────────────────────────────────────────────────────

// Pairs the scene's records with a reader's by name (assimp names a camera
// or light after its node: FBXConverter.cpp L399 and L476, ColladaLoader.cpp
// L337 and L407, glTF2Importer.cpp L1249-L1255), its own name first, then
// the names a reader gives a node with none (nameCandidates); each record
// pairs once.
function pairs(want, got) {
  const used = new Set();
  const take = (g) => {
    if (g) used.add(g);
    return g ?? null;
  };
  const out = want.map((w) => {
    const names = nameCandidates(w.name);
    return [w, take(got.find((g) => !used.has(g) && g.name === w.name) ?? got.find((g) => !used.has(g) && names.includes(g.name)))];
  });
  for (const g of got) if (!used.has(g)) out.push([null, g]);
  return out;
}

// The checks a comparison accumulates, and its verdict counts.
function checker() {
  const checks = [];
  return {
    checks,
    scalar: (subject, field, w, g) => checks.push({ subject, field, want: w, got: g, delta: Math.abs(g - w), verdict: within(g, w) ? 'pass' : 'fail' }),
    vector: (subject, field, w, g, extra = {}) => {
      const delta = Math.max(...w.map((x, i) => Math.abs(g[i] - x)));
      checks.push({ subject, field, want: w, got: g, delta, verdict: w.every((x, i) => within(g[i], x)) ? 'pass' : 'fail', ...extra });
    },
    direction: (subject, field, w, g) => {
      const a = angleBetween(w, g);
      checks.push({ subject, field, want: w, got: g, degrees: (a * 180) / Math.PI, verdict: a <= TOLERANCE ? 'pass' : 'fail' });
    },
    skip: (subject, field, reason) => checks.push({ subject, field, verdict: 'skip', reason }),
    result: () => {
      const count = (v) => checks.filter((c) => c.verdict === v).length;
      return { checks, passed: count('pass'), failed: count('fail'), skipped: count('skip') };
    },
  };
}

// The cameras' frames at sample time `key`: eye, forward and up of each
// camera both sides have, as `eye@<key>` and so on (compareScene reports a
// camera on one side only).
export function compareFrames(want, got, key) {
  const k = checker();
  for (const [w, g] of pairs(want || [], got || [])) {
    if (!w || !g || g.error) continue;
    const subject = `camera ${w.name}`;
    k.vector(subject, `eye@${key}`, w.eye, g.eye);
    k.direction(subject, `forward@${key}`, w.forward, g.forward);
    k.direction(subject, `up@${key}`, w.up, g.up);
  }
  return k.result();
}

// compareFrames at each of `times`: the scene's cameras ({ json, bin } of
// its GLB) at that time against `camerasAt(t)`, a reader's.
export function sampleTimeChecks(scene, camerasAt, times) {
  const checks = [];
  for (const t of times) checks.push(...compareFrames(sceneCameras(scene.json, t, scene.bin), camerasAt(t), String(t)).checks);
  const count = (v) => checks.filter((c) => c.verdict === v).length;
  return { checks, passed: count('pass'), failed: count('fail'), skipped: count('skip') };
}

// assjson writes no aiCamera::mOrthographicWidth (ASSIMP.glb.magSkip), so a
// glTF output's orthographic magnifications are read from its own JSON
// instead: the camera on the node assimp named it after (the node's name,
// or `nodes[i]` for one with none, glTF2Importer.cpp L1106-L1108).
export function withGltfMagnifications(cameras, json) {
  const byNode = new Map();
  (json.nodes || []).forEach((n, i) => {
    const name = n.name || `nodes[${i}]`;
    if (n.camera !== undefined && json.cameras?.[n.camera] && !byNode.has(name)) byNode.set(name, json.cameras[n.camera]);
  });
  return cameras.map((cam) => {
    const c = byNode.get(cam.name);
    if (cam.projection !== 'orthographic' || c?.type !== 'orthographic' || !c.orthographic) return cam;
    return { ...cam, xmag: c.orthographic.xmag, ymag: c.orthographic.ymag, magFrom: 'the file\'s own glTF JSON' };
  });
}

// Every check of `want` (the scene) against `got` (assimp's reading of the
// file, written as `format`): { subject, field, verdict: pass | fail | skip,
// want, got, delta | degrees, reason }.
export function compareScene(want, got, format) {
  const conv = format === 'usd' ? USD_TEXT : ASSIMP[format];
  const k = checker();
  const { checks, scalar, vector, direction, skip } = k;
  const missing = (subject, w, g, what) => {
    if (g?.error) return checks.push({ subject, field: 'present', verdict: 'fail', reason: g.error });
    if (!g) return checks.push({ subject, field: 'present', verdict: 'fail', reason: `assimp read no ${what} on ${w.name}` });
    return checks.push({ subject, field: 'present', verdict: 'fail', reason: `assimp read a ${what} on ${g.name}, which the scene does not have` });
  };

  for (const [w, g] of pairs(want.cameras || [], got.cameras || [])) {
    const subject = `camera ${(w || g).name}`;
    if (!w || !g || g.error) {
      missing(subject, w, g, 'camera');
      continue;
    }
    // A USD camera's frame is composed here from its xformOps; the citation
    // rides on its eye.
    vector(subject, 'eye', w.eye, g.eye, conv.xformCite ? { cite: conv.xformCite } : {});
    direction(subject, 'forward', w.forward, g.forward);
    direction(subject, 'up', w.up, g.up);
    if (conv.projection) checks.push({ subject, field: 'projection', want: w.projection, got: g.projection, verdict: w.projection === g.projection ? 'pass' : 'fail' });
    else skip(subject, 'projection', conv.projectionSkip);
    if (w.projection === 'perspective') {
      // A Collada camera with <yfov> alone: assimp's fov is its default, not
      // the file's (COLLADA_YFOV_ALONE), so the file's own number is judged.
      const o = format === 'dae' ? g.colladaOptics : undefined;
      if (o && o.yfov !== undefined && o.xfov === undefined && o.aspectRatio === undefined) {
        skip(subject, 'yfov', COLLADA_YFOV_ALONE);
        scalar(subject, "yfov (the file's own <yfov>)", w.yfov, (o.yfov * Math.PI) / 180);
      } else {
        scalar(subject, 'yfov', w.yfov, g.yfov);
      }
      if (w.aspect === undefined) skip(subject, 'aspect', 'the scene camera has no aspect ratio of its own (the viewport decides)');
      else scalar(subject, 'aspect', w.aspect, g.aspect);
    } else {
      skip(subject, 'yfov', 'an orthographic camera has no field of view');
      if (conv.orthoAspect) scalar(subject, 'aspect', w.aspect, g.aspect);
      else skip(subject, 'aspect', conv.orthoAspectSkip);
      // A glTF output's magnifications come from its own JSON
      // (withGltfMagnifications).
      if (conv.mag || (g.xmag !== undefined && g.ymag !== undefined)) {
        scalar(subject, 'xmag', w.xmag, g.xmag);
        scalar(subject, 'ymag', w.ymag, g.ymag);
      } else {
        skip(subject, 'xmag, ymag', conv.magSkip);
      }
    }
    scalar(subject, 'znear', w.znear, g.znear);
    if (w.zfar === undefined) skip(subject, 'zfar', 'the scene camera has an infinite far plane');
    else scalar(subject, 'zfar', w.zfar, g.zfar);
  }

  for (const [w, g] of pairs(want.lights || [], got.lights || [])) {
    const subject = `light ${(w || g).name}`;
    if (!w || !g || g.error) {
      missing(subject, w, g, 'light');
      continue;
    }
    checks.push({ subject, field: 'kind', want: w.kind, got: g.kind, verdict: w.kind === g.kind ? 'pass' : 'fail' });
    if (w.kind !== 'directional') vector(subject, 'position', w.position, g.position);
    if (w.kind !== 'point') {
      if (g.direction) direction(subject, 'direction', w.direction, g.direction);
      else checks.push({ subject, field: 'direction', verdict: 'fail', reason: `assimp gave ${g.kind} light ${g.name} no direction` });
    }
    const encode = conv.color === 'srgb' ? srgbEncode : (c) => c;
    vector(subject, 'color × intensity', w.color.map((c) => encode(c) * w.intensity), g.product, { cite: conv.colorCite });
    if (w.kind === 'spot' && g.kind === 'spot') {
      checks.push({ subject, field: 'inner cone', want: w.inner, got: g.inner, delta: Math.abs(g.inner - w.inner), verdict: within(g.inner, w.inner) ? 'pass' : 'fail', cite: conv.conesCite });
      checks.push({ subject, field: 'outer cone', want: w.outer, got: g.outer, delta: Math.abs(g.outer - w.outer), verdict: within(g.outer, w.outer) ? 'pass' : 'fail', cite: conv.conesCite });
    }
    if (w.kind !== 'directional') {
      // The scene's lights fall off by the inverse square, which assimp's
      // 1/(constant + linear·d + quadratic·d²) holds as 0, 0 and some q > 0.
      const [c, l, q] = g.attenuation;
      checks.push({ subject, field: 'falloff', want: 'inverse square (0, 0, q > 0)', got: g.attenuation, verdict: c === 0 && l === 0 && q > 0 ? 'pass' : 'fail', cite: conv.falloffCite });
    }
  }
  return k.result();
}

// ─── running assimp ─────────────────────────────────────────────────────────

// assimp 6.0.5 reads no optics for a Collada <camera> without a `name`
// attribute, which COLLADA 1.4.1 makes optional (collada_schema_1_4_1.xsd
// L2379-L2385, "Optional attribute"): ColladaParser.cpp L1037-L1058 creates
// the camera's library entry, then skips ReadCamera when the name is missing
// (`continue`, L1049-L1051), so the camera keeps Collada::Camera's defaults
// (no fov, near 0.1, far 1000). litegen's writer names a camera that has no
// name by its id since item 5's final fix wave (dae.rs `library_cameras`), so
// this now matters for other writers' files. assimp reads a copy in which each
// nameless <camera> is named after its id (oracleFile); nothing else differs,
// and assimp names a camera after its node, not this.
export const COLLADA_NAMELESS_CAMERA = `assimp v6.0.5 ColladaParser.cpp L1037-L1058 skips the optics of a <camera> with no name attribute (L1049-L1051), which COLLADA 1.4.1 makes optional (collada_schema_1_4_1.xsd L2379-L2385); assimp read a copy with each nameless <camera> named after its id`;
export function namedColladaCameras(text) {
  return text.replace(/<camera\b([^>]*?)(\/?)>/g, (m, attrs, close) => {
    const id = /\bid\s*=\s*"([^"]*)"/.exec(attrs);
    return !id || /\bname\s*=/.test(attrs) ? m : `<camera${attrs} name="${id[1]}"${close}>`;
  });
}

// A Collada <perspective> with <yfov> alone (item 5 follow-ups, 2026-11-08, B).
// COLLADA 1.4.1 allows it (the aspect ratio is then the application's), and
// litegen writes it for a camera with no aspect ratio (dae.rs `library_cameras`,
// L1373-L1378). assimp's ColladaLoader sets aiCamera::mHorizontalFOV from
// <xfov> (L422-L423), or from <yfov> with <aspect_ratio> (L430-L432), and
// otherwise leaves the aiCamera default of pi/4 (camera.h L187); L436 then
// turns that from degrees to radians, pi/4 x pi/180 = 0.0137078, whatever
// <yfov> says. The parser's own defaults are 10e10 for "not given"
// (ColladaHelper.h L123-L130), and ColladaParser.cpp L398-L403 reads <xfov>,
// <yfov> and <aspect_ratio>. So a camera with neither <xfov> nor
// <aspect_ratio> has no fov to read back through assimp.
export const COLLADA_YFOV_ALONE = `assimp v6.0.5 ColladaLoader.cpp L418-L436 sets a camera's horizontal fov from <xfov> (L422-L423) or from <yfov> with <aspect_ratio> (L430-L432), and otherwise leaves aiCamera's default of pi/4 (camera.h L187), which L436 turns from degrees to radians (pi/4 x pi/180 = 0.0137078); this camera has neither <xfov> nor <aspect_ratio> (<yfov> alone, which COLLADA 1.4.1 allows and litegen writes for a camera with no aspect ratio), so assimp's fov is not the file's; the file's own <yfov> is judged instead`;

// The optics each camera node of a Collada file has, by the node's id (assimp
// names a camera after its node's id, ColladaLoader.cpp L1801-L1822): for each
// <instance_camera> of a <perspective> camera, { yfov, xfov, aspectRatio }
// as written (degrees; undefined when the element is absent). Read from the
// file's own text, as namedColladaCameras reads it.
export function colladaCameraOptics(text) {
  const cameras = new Map();
  for (const m of text.matchAll(/<camera\b([^>]*)>([\s\S]*?)<\/camera>/g)) {
    const id = /\bid\s*=\s*"([^"]*)"/.exec(m[1])?.[1];
    const persp = /<perspective\b[^>]*>([\s\S]*?)<\/perspective>/.exec(m[2])?.[1];
    if (id === undefined || persp === undefined) continue;
    const num = (tag) => {
      const v = new RegExp(`<${tag}\\b[^>]*>\\s*([^<\\s]+)\\s*</${tag}>`).exec(persp)?.[1];
      return v === undefined ? undefined : Number(v);
    };
    cameras.set(id, { yfov: num('yfov'), xfov: num('xfov'), aspectRatio: num('aspect_ratio') });
  }
  const out = new Map();
  const open = [];
  for (const m of text.matchAll(/<(\/?)node\b([^>]*?)(\/?)>|<instance_camera\b[^>]*?\burl\s*=\s*"#([^"]*)"/g)) {
    if (m[4] !== undefined) {
      const optics = cameras.get(m[4]);
      const node = open[open.length - 1];
      if (optics && node !== undefined && !out.has(node)) out.set(node, optics);
    } else if (m[1]) {
      open.pop();
    } else if (!m[3]) {
      open.push(/\bid\s*=\s*"([^"]*)"/.exec(m[2])?.[1]);
    }
  }
  return out;
}

export const FORMAT_OF = { '.fbx': 'fbx', '.dae': 'dae', '.glb': 'glb', '.gltf': 'gltf' };

// `assimp export <file> <out> -fassjson`, read back.
export async function assjsonOf(file, out) {
  await fsp.mkdir(path.dirname(out), { recursive: true });
  try {
    await run('assimp', ['export', file, out, '-fassjson'], { timeout: 120000, maxBuffer: 64 << 20 });
  } catch (e) {
    const why = String(e.stdout || '').split(/[\r\n%]+/).filter((l) => l.includes('ERROR')).slice(-1);
    throw new Error(`assimp export failed: ${[String(e.stderr || e.message).trim().slice(0, 200), ...why].join(' / ')}`);
  }
  return parseAssjson(await fsp.readFile(out, 'utf8'));
}

// A comparison at rest and its checks at the sample times, as one.
function joined(r, f) {
  return { checks: [...r.checks, ...f.checks], passed: r.passed + f.passed, failed: r.failed + f.failed, skipped: r.skipped + f.skipped };
}
// The sample-time checks, or the error that stopped them.
function atTimes(scene, camerasAt, times) {
  try {
    return { ...sampleTimeChecks(scene, camerasAt, times) };
  } catch (e) {
    return { checks: [{ subject: 'cameras', field: 'frames at the sample times', verdict: 'fail', reason: String(e.message || e) }], passed: 0, failed: 1, skipped: 0 };
  }
}

// The oracle on one file: assimp's reading of `file` against `expectedGlb`,
// at rest and, for each camera's frame, at each of `times` (seconds).
export async function oracleFile(file, expectedGlb, out, times = []) {
  const format = FORMAT_OF[path.extname(file).toLowerCase()];
  if (!format) return { file, error: `the oracle reads no ${path.extname(file)} file` };
  const scene = readGlb(await fsp.readFile(expectedGlb));
  const want = { cameras: sceneCameras(scene.json), lights: sceneLights(scene.json) };
  let aj;
  let input = file;
  let note;
  let optics = new Map();
  if (format === 'dae') {
    const text = await fsp.readFile(file, 'utf8');
    optics = colladaCameraOptics(text);
    const named = namedColladaCameras(text);
    if (named !== text) {
      input = `${out}.dae`;
      await fsp.mkdir(path.dirname(input), { recursive: true });
      await fsp.writeFile(input, named);
      note = COLLADA_NAMELESS_CAMERA;
    }
  }
  try {
    aj = await assjsonOf(input, out);
  } catch (e) {
    return { file, format, error: String(e.message || e), checks: [], passed: 0, failed: 1, skipped: 0, want };
  }
  let cameras = assimpCameras(aj, format);
  if (format === 'glb' || format === 'gltf') cameras = withGltfMagnifications(cameras, readGltfJson(await fsp.readFile(file)));
  // A Collada camera carries the optics its file writes (COLLADA_YFOV_ALONE).
  if (format === 'dae') cameras = cameras.map((c) => (optics.has(c.name) ? { ...c, colladaOptics: optics.get(c.name) } : c));
  const got = { cameras, lights: assimpLights(aj, format) };
  const r = joined(compareScene(want, got, format), atTimes(scene, (t) => assimpCameras(aj, format, t), times));
  return { file, format, ...r, ...(note ? { note } : {}), want, got, aj };
}

// The USD check on one file: its cameras, read from `usdcat --flatten`,
// against `expectedGlb`'s, at rest and, for each camera's frame, at each of
// `times` (seconds).
export async function usdCameraFile(file, expectedGlb, times = []) {
  const scene = readGlb(await fsp.readFile(expectedGlb));
  const want = { cameras: sceneCameras(scene.json), lights: [] };
  let text;
  try {
    text = (await run('usdcat', ['--flatten', file], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  } catch (e) {
    return { file, format: 'usd', error: `usdcat --flatten failed: ${String(e.stderr || e.message).trim().slice(0, 200)}`, checks: [], passed: 0, failed: 1, skipped: 0, want };
  }
  const got = { cameras: usdCameras(text), lights: [] };
  const r = joined(compareScene(want, got, 'usd'), atTimes(scene, (t) => usdCameras(text, t), times));
  return { file, format: 'usd', ...r, want, got };
}

// What a judge needs to render through assimp's reading at each time (the
// FBX judge of verify.mjs's camera and lit_lights modes): each camera's eye,
// forward and up, and each light's position and direction, per time key
// ('static' for the rest pose, String(t) for a sample time), beside the
// values that do not move.
export function assimpView(aj, format, times) {
  const keys = [['static', null], ...times.map((t) => [String(t), t])];
  const at = keys.map(([k, t]) => [k, assimpCameras(aj, format, t), assimpLights(aj, format, t)]);
  const cameras = assimpCameras(aj, format).map((c, i) => ({
    ...c,
    frames: Object.fromEntries(at.map(([k, cs]) => [k, { eye: cs[i].eye, forward: cs[i].forward, up: cs[i].up }])),
  }));
  const lights = assimpLights(aj, format).map((l, i) => ({
    ...l,
    frames: Object.fromEntries(at.map(([k, , ls]) => [k, { position: ls[i].position, direction: ls[i].direction }])),
  }));
  return { cameras, lights };
}

// ─── CLI ────────────────────────────────────────────────────────────────────

function formatResult(r) {
  const lines = [`${r.error ? 'ERROR' : r.failed ? 'FAIL ' : 'ok   '} ${r.label}: ${r.passed ?? 0} pass, ${r.failed ?? 0} fail, ${r.skipped ?? 0} skipped${r.error ? ` — ${r.error}` : ''}`];
  if (r.note) lines.push(`    note: ${r.note}`);
  for (const c of r.checks || []) {
    if (c.verdict !== 'fail') continue;
    const said = c.degrees !== undefined ? `${c.degrees.toFixed(4)}° apart` : c.reason ?? `${JSON.stringify(c.want)} -> ${JSON.stringify(c.got)}`;
    lines.push(`    ${c.subject} / ${c.field}: ${said}${c.degrees !== undefined ? ` (want ${JSON.stringify(c.want)}, got ${JSON.stringify(c.got)})` : ''}`);
  }
  return lines.join('\n');
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const opt = (name) => {
    const i = args.indexOf(name);
    return i >= 0 ? args[i + 1] : undefined;
  };
  const jsonOut = opt('--json');
  const work = opt('--work') || fs.mkdtempSync(path.join(os.tmpdir(), 'camlight-oracle-'));
  const results = [];
  if (opt('--file')) {
    const r = await oracleFile(path.resolve(opt('--file')), path.resolve(opt('--expected')), path.join(work, 'file.assjson'));
    results.push({ label: opt('--file'), ...r });
  } else {
    const matrix = path.resolve(args[0] || '');
    if (!args[0] || args[0].startsWith('--')) {
      console.error('usage: node camlight-oracle.mjs <matrix_dir> [--fixtures a,b] [--json <file>] [--work <dir>]\n       node camlight-oracle.mjs --file <file> --expected <expected.glb> [--json <file>]');
      process.exit(2);
    }
    const manifest = JSON.parse(await fsp.readFile(path.join(matrix, 'manifest.json'), 'utf8'));
    const only = opt('--fixtures')?.split(',');
    for (const [name, fx] of Object.entries(manifest.fixtures)) {
      if (only && !only.includes(name)) continue;
      for (const [key, rel] of Object.entries(fx.expected)) {
        const json = readGltfJson(await fsp.readFile(path.join(matrix, rel)));
        if (!sceneCameras(json).length && !sceneLights(json).length) continue;
        const r = await oracleFile(path.join(matrix, rel), path.join(matrix, rel), path.join(work, name, 'expected', `${key}.assjson`), json.animations?.length ? fx.times || [] : []);
        results.push({ label: `${name} expected ${key} (calibration)`, ...r });
      }
    }
    for (const c of manifest.cases) {
      const usd = c.to === 'usdz' || c.to === 'usda' || c.to === 'usdc';
      if ((only && !only.includes(c.fixture)) || !(usd || ['fbx', 'dae', 'glb', 'gltf'].includes(c.to)) || !c.primary || c.error) continue;
      const expected = path.join(matrix, manifest.fixtures[c.fixture].expected[c.expected]);
      const times = c.caps.animation ? manifest.fixtures[c.fixture].times || [] : [];
      const r = usd
        ? await usdCameraFile(path.join(matrix, c.primary), expected, times)
        : await oracleFile(path.join(matrix, c.primary), expected, path.join(work, c.fixture, `${c.from}__${c.to}.assjson`), times);
      if (!r.error && !r.want.cameras.length && !r.want.lights.length && !r.got.cameras.length && !r.got.lights.length) continue;
      results.push({ label: `${c.fixture} ${c.from} -> ${c.to}`, ...r });
    }
  }
  for (const r of results) console.log(formatResult(r));
  const failed = results.filter((r) => r.error || r.failed).length;
  console.log(`\ncamlight oracle: ${results.length - failed} of ${results.length} file(s) agree with their scene within ${TOLERANCE}; ${results.reduce((s, r) => s + (r.skipped || 0), 0)} check(s) skipped with a citation`);
  if (jsonOut) fs.writeFileSync(jsonOut, JSON.stringify(results.map(({ aj, ...r }) => r), null, 1));
  process.exit(failed ? 1 : 0);
}
