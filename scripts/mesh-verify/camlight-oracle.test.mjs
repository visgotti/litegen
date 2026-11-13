// Tests for camlight-oracle.mjs, the numeric oracle for lights and cameras
// (spec "Render harness"; item 5's Ruling R11: assimp 6.0.5's assjson export,
// since its glTF2 exporter writes neither). Run, from the repo:
//
//   node --test scripts/mesh-verify/camlight-oracle.test.mjs
//
// Every expectation below is a hand-derived literal, or the value assimp
// itself wrote for litegen's `staged` fixture (`assimp export model.fbx
// x.assjson -fassjson`, 2026-11-06): assimp is the independent reader here,
// so its numbers are the evidence, never the oracle's own arithmetic. The
// assjson documents are written by hand in assimp's layout (row-major node
// matrices with the translation in the fourth column, json_exporter.cpp
// L268-L276; quaternions as [w, x, y, z], L251-L258).
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  TOLERANCE,
  assimpCameras,
  assimpLights,
  assimpWorld,
  colladaCameraOptics,
  colladaInnerCone,
  compareFrames,
  compareScene,
  mat4Dir,
  mat4Mul,
  mat4Point,
  nameCandidates,
  namedColladaCameras,
  parseAssjson,
  quatSlerp,
  readGlb,
  sampleTimeChecks,
  sceneCameras,
  sceneLights,
  srgbEncode,
  trsMatrix,
  usdCameras,
  withGltfMagnifications,
} from './camlight-oracle.mjs';
import { depthFirstRanks, unnamedKey } from './page/node-rank.js';
import { cameraViewFor, externalPictureSize, UNSET_ASPECT } from './page/camera-view.js';
import { colladaClips, sceneKitClip, SCENEKIT_CLIP_SHADOWED, SCENEKIT_SHARED_CLIPS, SCENEKIT_USD_TEXSCALE, usdTextureTint } from './scenekit-limits.mjs';

const H = Math.SQRT1_2; // sin 45° = cos 45°
const I4 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
const near = (got, want, tol = 1e-9, what = '') => assert.ok(Math.abs(got - want) <= tol, `${what} ${got} is not within ${tol} of ${want}`);
const nearVec = (got, want, tol = 1e-9, what = '') => {
  assert.ok(Array.isArray(got), `${what} is not a vector: ${JSON.stringify(got)}`);
  assert.equal(got.length, want.length, `${what} length`);
  got.forEach((g, i) => near(g, want[i], tol, `${what}[${i}]`));
};
// A row-major node matrix: rotation rows r0..r2, translation t.
const node = (r0, r1, r2, t) => [...r0, t[0], ...r1, t[1], ...r2, t[2], 0, 0, 0, 1];
// R_y(+90°) takes +X to -Z: rows [0 0 1], [0 1 0], [-1 0 0].
const RY90 = [[0, 0, 1], [0, 1, 0], [-1, 0, 0]];
// R_x(+90°) takes -Y to -Z: rows [1 0 0], [0 0 -1], [0 1 0].
const RX90 = [[1, 0, 0], [0, 0, -1], [0, 1, 0]];
const ID3 = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
// A named check of a comparison, asserted present.
const check = (r, subject, field) => {
  const c = r.checks.find((x) => x.subject === subject && x.field === field);
  assert.ok(c, `no ${field} check on ${subject}`);
  return c;
};

// ─── the maths ──────────────────────────────────────────────────────────────

test('a child matrix composes under its parent, assimp row-major', () => {
  const parent = node(...ID3, [1, 2, 3]);
  const child = trsMatrix([0, 0, 0], [0, H, 0, H], [1, 1, 1]);
  nearVec(child, node(...RY90, [0, 0, 0]), 1e-12, 'R_y(90°)');
  const world = mat4Mul(parent, child);
  nearVec(mat4Point(world, [1, 0, 0]), [1, 2, 2], 1e-12, 'point');
  nearVec(mat4Dir(world, [1, 0, 0]), [0, 0, -1], 1e-12, 'direction');
});

test('quaternions slerp halfway to half the angle', () => {
  nearVec(quatSlerp([0, 0, 0, 1], [0, H, 0, H], 0.5), [0, 0.3826834323650898, 0, 0.9238795325112867], 1e-12, 'slerp');
});

test('sRGB encoding follows IEC 61966-2-1 on both branches', () => {
  // 0.735357 is also what SceneKit reports for litegen's sRGB-written 0.5.
  near(srgbEncode(0.5), 0.7353569830524495, 1e-9, 'enc(0.5)');
  near(srgbEncode(0.002), 0.02584, 1e-12, 'enc(0.002)');
});

test('assjson special floats are read back as numbers, names left alone', () => {
  const d = parseAssjson('{"name": "NaN", "a": "NaN", "b": "Infinity", "c": ["-Infinity", 1]}');
  assert.equal(d.name, 'NaN');
  assert.ok(Number.isNaN(d.a));
  assert.equal(d.b, Infinity);
  assert.deepEqual(d.c, [-Infinity, 1]);
});

test("assimp's first animation is evaluated at a time, keys in ticks", () => {
  const aj = {
    rootnode: { name: 'RootNode', transformation: I4, children: [{ name: 'rig', transformation: I4 }] },
    animations: [{ name: 'Orbit', tickspersecond: 30, duration: 60, channels: [{
      name: 'rig',
      positionkeys: [[0, [0, 0, 0]], [60, [0, 0, 0]]],
      rotationkeys: [[0, [1, 0, 0, 0]], [60, [H, 0, H, 0]]],
      scalingkeys: [[0, [1, 1, 1]], [60, [1, 1, 1]]],
    }] }],
  };
  // At rest the node's own matrix; at 1 s (30 ticks) half the quarter turn.
  nearVec(mat4Dir(assimpWorld(aj, null).get('rig'), [1, 0, 0]), [1, 0, 0], 1e-12, 'rest');
  nearVec(mat4Dir(assimpWorld(aj, 1).get('rig'), [1, 0, 0]), [H, 0, -H], 1e-12, 't = 1 s');
  // Past the last key, the last key holds.
  nearVec(mat4Dir(assimpWorld(aj, 5).get('rig'), [1, 0, 0]), [0, 0, -1], 1e-12, 't = 5 s');
});

test("a channel without scaling keys still moves its node, as assimp's glTF importer writes one", () => {
  // glTF2Importer.cpp CreateNodeAnim gives a component with no sampler one
  // key of the node's own value, and none where the node writes no value.
  const aj = {
    rootnode: { name: 'RootNode', transformation: I4, children: [{ name: 'rig', transformation: I4 }] },
    animations: [{ name: 'Orbit', tickspersecond: 1000, duration: 2000, channels: [{
      name: 'rig',
      positionkeys: [[0, [0, 0, 0]]],
      rotationkeys: [[0, [1, 0, 0, 0]], [2000, [H, 0, H, 0]]],
      scalingkeys: [],
    }] }],
  };
  nearVec(mat4Dir(assimpWorld(aj, 1).get('rig'), [1, 0, 0]), [H, 0, -H], 1e-12, 't = 1 s');
});

// ─── cameras ────────────────────────────────────────────────────────────────

// litegen's staged persp camera as assimp read it from litegen's FBX: a
// half horizontal fov (FBXConverter.cpp L501-L504), looking along the model's
// +X (L483), on a model turned by R_y(+90°) so that +X is the scene's -Z.
const fbxCamera = (rows = RY90) => ({
  rootnode: { name: 'RootNode', transformation: I4, children: [
    { name: 'rig', transformation: node(...ID3, [0.5, 0, -0.25]), children: [
      { name: 'persp_camera_node', transformation: node(...rows, [0, 1.5, 4]) },
    ] },
  ] },
  cameras: [{ name: 'persp_camera_node', aspect: 1.5, clipplanefar: 64, clipplanenear: 0.125, horizontalfov: 0.56518, up: [0, 1, 0], lookat: [1, 0, 0] }],
  lights: [],
});
const sceneCamera = {
  name: 'persp_camera_node', eye: [0.5, 1.5, 3.75], forward: [0, 0, -1], up: [0, 1, 0],
  projection: 'perspective', yfov: 0.8, aspect: 1.5, znear: 0.125, zfar: 64,
};

test("an FBX camera reads back through assimp's half fov and +X look-at", () => {
  const [c] = assimpCameras(fbxCamera(), 'fbx');
  assert.equal(c.name, 'persp_camera_node');
  nearVec(c.eye, [0.5, 1.5, 3.75], 1e-9, 'eye');
  nearVec(c.forward, [0, 0, -1], 1e-9, 'forward');
  nearVec(c.up, [0, 1, 0], 1e-9, 'up');
  near(c.yfov, 0.8, TOLERANCE, 'yfov');
  near(c.aspect, 1.5, 0, 'aspect');
  near(c.znear, 0.125, 0, 'znear');
  near(c.zfar, 64, 0, 'zfar');
});

test("a Collada or glTF camera reads back through assimp's full fov and -Z look-at", () => {
  for (const format of ['dae', 'glb']) {
    const aj = fbxCamera(ID3);
    aj.cameras[0].horizontalfov = 1.13036;
    aj.cameras[0].lookat = [0, 0, -1];
    const [c] = assimpCameras(aj, format);
    nearVec(c.forward, [0, 0, -1], 1e-9, `${format} forward`);
    near(c.yfov, 0.8, TOLERANCE, `${format} yfov`);
  }
});

test('the scene camera matches its FBX reading, check by check', () => {
  const r = compareScene({ cameras: [sceneCamera], lights: [] }, { cameras: assimpCameras(fbxCamera(), 'fbx'), lights: [] }, 'fbx');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  for (const f of ['eye', 'forward', 'up', 'yfov', 'aspect', 'znear', 'zfar']) {
    assert.equal(check(r, 'camera persp_camera_node', f)?.verdict, 'pass', f);
  }
});

test('an FBX camera written without its quarter turn fails forward by 90°', () => {
  // The negative control's file: the model keeps the scene's rotation, so
  // assimp's +X look-at is the scene's +X, not its -Z.
  const r = compareScene({ cameras: [sceneCamera], lights: [] }, { cameras: assimpCameras(fbxCamera(ID3), 'fbx'), lights: [] }, 'fbx');
  const f = check(r, 'camera persp_camera_node', 'forward');
  assert.equal(f.verdict, 'fail');
  near(f.degrees, 90, 1e-9, 'forward apart');
  assert.ok(r.failed >= 1);
});

test("an orthographic FBX camera has its projection skipped, citing assimp's converter", () => {
  const aj = fbxCamera();
  aj.cameras[0] = { ...aj.cameras[0], name: 'persp_camera_node', horizontalfov: 1.51372 };
  const ortho = { ...sceneCamera, projection: 'orthographic', yfov: undefined, xmag: 3, ymag: 2, aspect: 1.5 };
  const r = compareScene({ cameras: [ortho], lights: [] }, { cameras: assimpCameras(aj, 'fbx'), lights: [] }, 'fbx');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  const p = check(r, 'camera persp_camera_node', 'projection');
  assert.equal(p.verdict, 'skip');
  assert.match(p.reason, /FBXConverter\.cpp/);
  assert.equal(check(r, 'camera persp_camera_node', 'aspect').verdict, 'pass');
});

test("glTF's orthographic camera reads as assimp's zero fov with xmag/ymag as its aspect", () => {
  const aj = fbxCamera(ID3);
  aj.cameras[0] = { ...aj.cameras[0], horizontalfov: 0, lookat: [0, 0, -1] };
  const [c] = assimpCameras(aj, 'glb');
  assert.equal(c.projection, 'orthographic');
  const ortho = { ...sceneCamera, projection: 'orthographic', yfov: undefined, xmag: 3, ymag: 2, aspect: 1.5 };
  const r = compareScene({ cameras: [ortho], lights: [] }, { cameras: [c], lights: [] }, 'glb');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((x) => x.verdict === 'fail')));
  assert.equal(check(r, 'camera persp_camera_node', 'projection').verdict, 'pass');
});

test('a camera on one side only fails, named', () => {
  const r = compareScene({ cameras: [sceneCamera], lights: [] }, { cameras: [], lights: [] }, 'fbx');
  assert.equal(r.failed, 1);
  assert.match(r.checks[0].reason, /persp_camera_node/);
  const extra = compareScene({ cameras: [], lights: [] }, { cameras: assimpCameras(fbxCamera(), 'fbx'), lights: [] }, 'fbx');
  assert.equal(extra.failed, 1);
  assert.match(extra.checks[0].reason, /persp_camera_node/);
});

// ─── lights ─────────────────────────────────────────────────────────────────

// staged's sun, at the scene's identity rotation, so it shines along -Z. In
// FBX its model is turned by R_x(+90°) and assimp aims it along the model's
// -Y (FBXConverter.cpp L411-L414). assimp multiplies Intensity/100 into the
// colour (L401-L407), which litegen wrote sRGB-encoded (Ruling R14): these
// are the numbers assimp wrote for staged's sun, colour (1, 0.875, 0.75) at
// 3 lux.
const fbxLights = (over = {}) => ({
  rootnode: { name: 'RootNode', transformation: I4, children: [
    { name: 'sun_node', transformation: node(...RX90, [2, 4, 3]) },
    { name: 'spot_node', transformation: node(...RX90, [1.5, 2.5, 1.5]) },
    { name: 'bulb_node', transformation: node(...RX90, [-1.5, 2, 1]) },
  ] },
  cameras: [],
  lights: [
    { name: 'sun_node', type: 1, attenuationconstant: 0, attenuationlinear: 0, attenuationquadratic: 2, diffusecolor: [3, 2.82871, 2.64248], direction: [0, -1, 0], ...over.sun },
    { name: 'spot_node', type: 3, angleinnercone: 0.4, angleoutercone: 1.2, attenuationconstant: 0, attenuationlinear: 0, attenuationquadratic: 2, diffusecolor: [29.4143, 35.233, 40], direction: [0, -1, 0], position: [0, 0, 0], ...over.spot },
    { name: 'bulb_node', type: 2, attenuationconstant: 0, attenuationlinear: 0, attenuationquadratic: 2, diffusecolor: [20, 17.6165, 14.7071], position: [0, 0, 0], ...over.bulb },
  ],
});
const sceneSun = { name: 'sun_node', kind: 'directional', position: [2, 4, 3], direction: [0, 0, -1], color: [1, 0.875, 0.75], intensity: 3 };
const sceneSpot = { name: 'spot_node', kind: 'spot', position: [1.5, 2.5, 1.5], direction: [0, 0, -1], color: [0.5, 0.75, 1], intensity: 40, inner: 0.2, outer: 0.6 };
const sceneBulb = { name: 'bulb_node', kind: 'point', position: [-1.5, 2, 1], color: [1, 0.75, 0.5], intensity: 20 };
const sceneLightsAll = [sceneSun, sceneSpot, sceneBulb];

test('FBX lights match through the turn, the sRGB colour and the full cones', () => {
  const got = assimpLights(fbxLights(), 'fbx');
  const sun = got.find((l) => l.name === 'sun_node');
  assert.ok(sun, 'assimp read no light on sun_node');
  nearVec(sun.direction, [0, 0, -1], 1e-9, 'sun direction');
  const spot = got.find((l) => l.name === 'spot_node');
  assert.ok(spot, 'assimp read no light on spot_node');
  near(spot.inner, 0.2, 1e-12, 'spot inner');
  near(spot.outer, 0.6, 1e-12, 'spot outer');
  const r = compareScene({ cameras: [], lights: sceneLightsAll }, { cameras: [], lights: got }, 'fbx');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  for (const f of ['kind', 'direction', 'color × intensity']) assert.equal(check(r, 'light sun_node', f)?.verdict, 'pass', f);
  for (const f of ['kind', 'position', 'direction', 'color × intensity', 'inner cone', 'outer cone', 'falloff']) {
    assert.equal(check(r, 'light spot_node', f)?.verdict, 'pass', f);
  }
  assert.equal(check(r, 'light bulb_node', 'position')?.verdict, 'pass');
});

test('an FBX light colour written linear, where the matrix writes sRGB, fails', () => {
  const r = compareScene({ cameras: [], lights: [sceneSun] }, { cameras: [], lights: assimpLights(fbxLights({ sun: { diffusecolor: [3, 2.625, 2.25] } }), 'fbx') }, 'fbx');
  assert.equal(check(r, 'light sun_node', 'color × intensity').verdict, 'fail');
});

test('an FBX light without the inverse square fails its falloff', () => {
  // DecayType 0 reads as constant DecayStart, linear 0, quadratic 0
  // (FBXConverter.cpp L447-L451).
  const r = compareScene({ cameras: [], lights: [sceneBulb] }, { cameras: [], lights: assimpLights(fbxLights({ bulb: { attenuationconstant: 1, attenuationquadratic: 0 } }), 'fbx') }, 'fbx');
  assert.equal(check(r, 'light bulb_node', 'falloff').verdict, 'fail');
});

test("a Collada spot's cones come back through assimp's guess and the documented rule", () => {
  // <falloff_angle> is the full cone, 2·deg(outer) (Ruling R23). assimp
  // takes it as its inner cone and guesses its outer one from
  // falloff_exponent (ColladaLoader.cpp L357-L372): 1.2 and 1.84486 are what
  // it wrote for staged's 0.2 / 0.6 spot (2026-11-06, the R23 writer).
  near(colladaInnerCone(1.2, 1.84486), 0.2, TOLERANCE, 'inner');
  const aj = {
    rootnode: { name: 'Scene', transformation: I4, children: [{ name: 'spot_node', transformation: node(...ID3, [1.5, 2.5, 1.5]) }] },
    cameras: [],
    lights: [{ name: 'spot_node', type: 3, angleinnercone: 1.2, angleoutercone: 1.84486, attenuationconstant: 0, attenuationlinear: 0, attenuationquadratic: 1, diffusecolor: [20, 30, 40], direction: [0, 0, -1], position: [0, 0, 0] }],
  };
  const got = assimpLights(aj, 'dae')[0];
  near(got.outer, 0.6, 1e-12, 'outer');
  near(got.inner, 0.2, TOLERANCE, 'inner');
  const r = compareScene({ cameras: [], lights: [sceneSpot] }, { cameras: [], lights: assimpLights(aj, 'dae') }, 'dae');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  // falloff_angle written as the half-angle (34.377°, the old Ruling R4)
  // reads as an outer cone of 0.3.
  aj.lights[0].angleinnercone = 0.6;
  aj.lights[0].angleoutercone = 0.6 + 0.64486;
  const wrong = compareScene({ cameras: [], lights: [sceneSpot] }, { cameras: [], lights: assimpLights(aj, 'dae') }, 'dae');
  assert.equal(check(wrong, 'light spot_node', 'outer cone').verdict, 'fail');
  near(check(wrong, 'light spot_node', 'outer cone').got, 0.3, 1e-12, 'half-angle file outer');
});

// ─── USD cameras, read from the flattened stage's text ──────────────────────

// staged's rig and two cameras as litegen's USD writer writes them, in usdcat
// --flatten's layout and f32 values: the rig's orient sampled at 0, 45 and
// 90° about +Y, 30 time codes a second. The litegen customData is there to
// show it is not read.
const ORTHO_OPS = `            float3 xformOp:translate = (-3, 2.5, 3)
            quatf xformOp:orient = (0.9015602, -0.20184809, -0.37343845, -0.08360822)`;
const usda = ({ clipping = '            float2 clippingRange = (0.125, 64)', ortho = ORTHO_OPS } = {}) => `#usda 1.0
(
    metersPerUnit = 1
    timeCodesPerSecond = 30
    upAxis = "Y"
)

def Xform "staged"
{
    def Xform "rig"
    {
        quatf xformOp:orient = (1, 0, 0, 0)
        quatf xformOp:orient.timeSamples = {
            0: (1, 0, 0, 0),
            30: (0.9238795, 0, 0.38268343, 0),
            60: (0.70710677, 0, 0.70710677, 0),
        }
        float3 xformOp:translate = (0.5, 0, -0.25)
        uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]

        def Camera "persp_camera_node" (
            customData = {
                dictionary litegen = {
                    float yfov = 0.5
                    float2 clippingRange = (9, 99)
                    float3 xformOp:translate = (9, 9, 9)
                }
            }
        )
        {
${clipping}
            float focalLength = 0.28382668
            float horizontalAperture = 0.36
            token projection = "perspective"
            float verticalAperture = 0.24
            quatf xformOp:orient = (0.9925076, -0.12218326, 0, 0)
            float3 xformOp:translate = (0, 1.5, 4)
            uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]
        }

        def Camera "ortho_camera_node"
        {
            float2 clippingRange = (0.0625, 20)
            float horizontalAperture = 60
            token projection = "orthographic"
            float verticalAperture = 40
${ortho}
            uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:orient"]
        }
    }
}
`;
// staged's cameras, hand-derived: each at its eye in the rig's space, aimed
// at (0, 0.5, 0) there, the rig at (0.5, 0, -0.25): forward the unit
// (target - eye), up world +Y made orthogonal to it.
const s17 = Math.sqrt(17);
const s22 = Math.sqrt(22);
const s11 = Math.sqrt(11);
const usdPersp = {
  name: 'persp_camera_node', eye: [0.5, 1.5, 3.75], forward: [0, -1 / s17, -4 / s17], up: [0, 4 / s17, -1 / s17],
  projection: 'perspective', yfov: 0.8, aspect: 1.5, znear: 0.125, zfar: 64,
};
const usdOrtho = {
  name: 'ortho_camera_node', eye: [-2.5, 2.5, 2.75], forward: [3 / s22, -2 / s22, -3 / s22], up: [1 / s11, 3 / s11, -1 / s11],
  projection: 'orthographic', xmag: 3, ymag: 2, aspect: 1.5, znear: 0.0625, zfar: 20,
};
// R_y(θ) of a vector: (x cos θ + z sin θ, y, -x sin θ + z cos θ).
const turnY = (deg) => ([x, y, z]) => {
  const t = (deg * Math.PI) / 180;
  return [x * Math.cos(t) + z * Math.sin(t), y, -x * Math.sin(t) + z * Math.cos(t)];
};

test("USD cameras read as UsdGeomCamera defines them: tenths of a unit, a range in units", () => {
  const [p, o] = usdCameras(usda());
  assert.deepEqual([p.name, p.path, p.projection], ['persp_camera_node', '/staged/rig/persp_camera_node', 'perspective']);
  // yfov = 2·atan(0.24 / (2 · 0.28382668)), the aspect 0.36 / 0.24.
  near(p.yfov, 0.8, 1e-6, 'yfov');
  near(p.aspect, 1.5, 1e-6, 'aspect');
  assert.deepEqual([p.znear, p.zfar], [0.125, 64]);
  // An orthographic aperture is the full width in tenths of a unit: 60 is 6
  // units across, an xmag of 3.
  assert.deepEqual([o.projection, o.xmag, o.ymag, o.aspect, o.znear, o.zfar], ['orthographic', 3, 2, 1.5, 0.0625, 20]);
});

test("a USD camera's frame is its prims' xformOps composed, at rest and at a time", () => {
  const [p, o] = usdCameras(usda());
  nearVec(p.eye, usdPersp.eye, 1e-6, 'persp eye');
  nearVec(p.forward, usdPersp.forward, 1e-6, 'persp forward');
  nearVec(p.up, usdPersp.up, 1e-6, 'persp up');
  nearVec(o.eye, usdOrtho.eye, 1e-6, 'ortho eye');
  nearVec(o.forward, usdOrtho.forward, 1e-6, 'ortho forward');
  nearVec(o.up, usdOrtho.up, 1e-6, 'ortho up');
  // At 1 s, time code 30, the rig is turned 45° about +Y; at 0.5 s halfway by
  // slerp, 22.5°; past the last sample it holds 90°.
  const at = (sec) => usdCameras(usda(), sec)[0];
  nearVec(at(1).eye, [0.5 + 4 * Math.SQRT1_2, 1.5, -0.25 + 4 * Math.SQRT1_2], 1e-6, 't = 1 eye');
  nearVec(at(1).forward, turnY(45)(usdPersp.forward), 1e-6, 't = 1 forward');
  nearVec(at(0.5).eye, [0.5 + 4 * Math.sin(Math.PI / 8), 1.5, -0.25 + 4 * Math.cos(Math.PI / 8)], 1e-6, 't = 0.5 eye');
  nearVec(at(5).eye, [4.5, 1.5, -0.25], 1e-6, 't = 5 eye');
});

test('a matrix op, a Z-up stage in centimetres and a reset stack compose too', () => {
  const text = `#usda 1.0
(
    metersPerUnit = 0.01
    upAxis = "Z"
)

def Xform "root"
{
    matrix4d xformOp:transform = ( (1, 0, 0, 0), (0, 1, 0, 0), (0, 0, 1, 0), (500, 0, 0, 1) )
    uniform token[] xformOpOrder = ["xformOp:transform"]

    def Camera "cam"
    {
        float3 xformOp:translate = (0, 0, 100)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def Camera "alone"
    {
        float3 xformOp:translate = (0, 0, 100)
        uniform token[] xformOpOrder = ["!resetXformStack!", "xformOp:translate"]
    }
}
`;
  const [cam, alone] = usdCameras(text);
  // The matrix's last row is its translation (USD's row vectors): 500 cm
  // along X. Z up becomes Y up, (x, z, -y), and a centimetre is 0.01 m.
  nearVec(cam.eye, [5, 1, 0], 1e-9, 'eye');
  nearVec(cam.forward, [0, -1, 0], 1e-9, 'forward');
  nearVec(cam.up, [0, 0, -1], 1e-9, 'up');
  // !resetXformStack! drops the parent's matrix.
  nearVec(alone.eye, [0, 1, 0], 1e-9, 'reset eye');
});

test('USD cameras match the scene, frames included, and a moved camera fails', () => {
  const r = compareScene({ cameras: [usdPersp, usdOrtho], lights: [] }, { cameras: usdCameras(usda()), lights: [] }, 'usd');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  for (const f of ['eye', 'forward', 'up', 'projection', 'yfov', 'aspect', 'znear', 'zfar']) assert.equal(check(r, 'camera persp_camera_node', f).verdict, 'pass', f);
  for (const f of ['eye', 'forward', 'up', 'projection', 'xmag', 'ymag', 'aspect', 'znear', 'zfar']) assert.equal(check(r, 'camera ortho_camera_node', f).verdict, 'pass', f);
  // The review's file: the second camera moved to (3, 0.5, -3) and unturned.
  const moved = usdCameras(usda({ ortho: '            float3 xformOp:translate = (3, 0.5, -3)\n            quatf xformOp:orient = (1, 0, 0, 0)' }));
  const bad = compareScene({ cameras: [usdPersp, usdOrtho], lights: [] }, { cameras: moved, lights: [] }, 'usd');
  assert.equal(check(bad, 'camera ortho_camera_node', 'eye').verdict, 'fail');
  assert.equal(check(bad, 'camera ortho_camera_node', 'forward').verdict, 'fail');
});

test('a camera frame is compared at each sample time too', () => {
  const at1 = { name: 'persp_camera_node', eye: [0.5 + 4 * Math.SQRT1_2, 1.5, -0.25 + 4 * Math.SQRT1_2], forward: turnY(45)(usdPersp.forward), up: turnY(45)(usdPersp.up) };
  const r = compareFrames([at1], usdCameras(usda(), 1), '1');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  for (const f of ['eye@1', 'forward@1', 'up@1']) assert.equal(check(r, 'camera persp_camera_node', f).verdict, 'pass', f);
  // A rig that holds still is 45° off at 1 s.
  const still = usda().replace(/        quatf xformOp:orient\.timeSamples = \{[^}]*\}\n/, '');
  const r2 = compareFrames([at1], usdCameras(still, 1), '1');
  assert.equal(check(r2, 'camera persp_camera_node', 'eye@1').verdict, 'fail');
  const apart = (Math.acos((1 + 16 * Math.SQRT1_2) / 17) * 180) / Math.PI;
  near(check(r2, 'camera persp_camera_node', 'forward@1').degrees, apart, 1e-4, 'still forward apart');
});

test('a USD stage is checked at each sample time against the scene at that time', () => {
  const scene = animatedScene();
  const r = sampleTimeChecks(scene, (t) => usdCameras(usda(), t), [0.34, 1, 1.66, 2]);
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  assert.equal(r.passed, 12);
  for (const t of ['0.34', '1', '1.66', '2']) assert.equal(check(r, 'camera persp_camera_node', `eye@${t}`).verdict, 'pass', t);
  const still = usda().replace(/        quatf xformOp:orient\.timeSamples = \{[^}]*\}\n/, '');
  const bad = sampleTimeChecks(scene, (t) => usdCameras(still, t), [0.34, 1]);
  assert.equal(check(bad, 'camera persp_camera_node', 'eye@1').verdict, 'fail');
  assert.equal(check(bad, 'camera persp_camera_node', 'forward@0.34').verdict, 'fail');
});

test('a USD near plane written wrong fails, though no picture shows it', () => {
  const r = compareScene({ cameras: [usdPersp], lights: [] }, { cameras: usdCameras(usda({ clipping: '            float2 clippingRange = (2, 64)' })), lights: [] }, 'usd');
  assert.equal(check(r, 'camera persp_camera_node', 'znear').verdict, 'fail');
});

test('a USD camera with no clippingRange takes the schema default and fails', () => {
  const r = compareScene({ cameras: [usdPersp], lights: [] }, { cameras: usdCameras(usda({ clipping: '' })), lights: [] }, 'usd');
  assert.equal(check(r, 'camera persp_camera_node', 'znear').verdict, 'fail');
  assert.equal(check(r, 'camera persp_camera_node', 'znear').got, 1);
  assert.equal(check(r, 'camera persp_camera_node', 'zfar').got, 1000000);
});

// ─── unnamed nodes, and a glTF output's own magnifications ──────────────────

test("unnamed nodes pair by their rank, across every reader's name for them", () => {
  assert.deepEqual(nameCandidates('node 3'), ['node 3', 'node3', 'node_3', 'nodes[3]']);
  assert.deepEqual(nameCandidates('persp_camera_node'), ['persp_camera_node']);
  // assimp's glTF importer names an unnamed node nodes[i]; litegen's FBX and
  // Collada writers write node{i}, its USD writer node_{i}.
  for (const [format, name] of [['glb', 'nodes[3]'], ['fbx', 'node3']]) {
    const aj = fbxCamera(format === 'fbx' ? RY90 : ID3);
    aj.rootnode.children[0].children[0].name = name;
    aj.cameras[0].name = name;
    if (format === 'glb') aj.cameras[0] = { ...aj.cameras[0], horizontalfov: 1.13036, lookat: [0, 0, -1] };
    const r = compareScene({ cameras: [{ ...sceneCamera, name: 'node 3' }], lights: [] }, { cameras: assimpCameras(aj, format), lights: [] }, format);
    assert.equal(r.failed, 0, `${format}: ${JSON.stringify(r.checks.filter((c) => c.verdict === 'fail'))}`);
    assert.equal(check(r, 'camera node 3', 'eye').verdict, 'pass', format);
  }
  const usd = usdCameras(usda().replace('"persp_camera_node"', '"node_3"'));
  const r = compareScene({ cameras: [{ ...usdPersp, name: 'node 3' }], lights: [] }, { cameras: [usd[0]], lights: [] }, 'usd');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
});

// Followups 2026-11-08, A: an unnamed node pairs by its depth-first rank
// (pre-order, the scene's roots in file order, children in file order), the
// order litegen's glTF reader numbers a scene's nodes in (glb.rs `nodes`,
// L2794-L2864) and its writers keep. Khronos's Duck has three unnamed nodes,
// the root's children [2, 1]: the camera is node 1 in the file and the third
// node in depth-first order, so litegen writes it as node 2.
const duckCamera = { type: 'perspective', perspective: { yfov: 0.66, aspectRatio: 1.5, znear: 1, zfar: 10000 } };
const duckFile = {
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ children: [2, 1] }, { camera: 0, translation: [0, 0, 5] }, { mesh: 0 }],
  cameras: [duckCamera],
};
const duckOurs = {
  scene: 0,
  scenes: [{ nodes: [0] }],
  nodes: [{ children: [1, 2] }, { mesh: 0 }, { camera: 0, translation: [0, 0, 5] }],
  cameras: [duckCamera],
};

test('depth-first ranks: roots in file order, children in file order, pre-order', () => {
  assert.deepEqual(depthFirstRanks(duckFile), [0, 2, 1]);
  assert.deepEqual(depthFirstRanks(duckOurs), [0, 1, 2], 'a file already in depth-first order is its own ranking');
  // Two roots [3, 0]; 3 has children [1, 2]; 4 hangs off no scene.
  const forest = {
    scenes: [{ nodes: [3, 0] }],
    nodes: [{}, { children: [4] }, {}, { children: [1, 2] }, {}],
  };
  assert.deepEqual(depthFirstRanks(forest), [4, 1, 3, 0, 2]);
  // Only the default scene is walked: `scene` names it, else the first.
  const two = { scene: 1, scenes: [{ nodes: [0] }, { nodes: [2, 1] }], nodes: [{}, {}, {}] };
  assert.deepEqual(depthFirstRanks(two), [undefined, 1, 0], 'the second scene is the default; node 0 is not reached');
  assert.deepEqual(depthFirstRanks({ scenes: [{ nodes: [1] }], nodes: [{}, {}] }), [undefined, 0]);
  // A node reached twice keeps its first rank; a missing node is skipped.
  assert.deepEqual(depthFirstRanks({ scenes: [{ nodes: [0, 0, 9] }], nodes: [{ children: [0] }] }), [0]);
  assert.deepEqual(depthFirstRanks({}), []);
});

test('an unnamed node is keyed by its rank, falling back to its index off the scene', () => {
  const ranks = depthFirstRanks(duckFile);
  assert.equal(unnamedKey(ranks, 1), 'node 2');
  assert.equal(unnamedKey(ranks, 2), 'node 1');
  assert.equal(unnamedKey([undefined, 0], 0), 'node 0', 'a node the default scene does not reach keeps its index');
});

test("Duck's unnamed camera pairs with litegen's reading of it, though the node indices differ", () => {
  const [mine] = sceneCameras(duckFile);
  const [ours] = sceneCameras(duckOurs);
  assert.equal(mine.name, 'node 2', 'node 1 in the file, third in depth-first order');
  assert.equal(ours.name, 'node 2');
  // By index they would not pair at all: the original's camera is on node 1
  // and ours is on node 2, so each would be left without a partner.
  const r = compareFrames(sceneCameras(duckFile), sceneCameras(duckOurs), 'static');
  assert.deepEqual(r.checks.map((c) => c.field), ['eye@static', 'forward@static', 'up@static']);
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  // And the numeric oracle's name candidates reach litegen's writers' names
  // for it (node{i} in FBX and Collada, node_{i} in USD, nodes[i] in glTF).
  assert.deepEqual(nameCandidates(mine.name), ['node 2', 'node2', 'node_2', 'nodes[2]']);
});

test('an unnamed camera moved to another node does not pair, so its judge fails', () => {
  // Negative control (followups 2026-11-08, A): the camera of our reading
  // placed on the mesh node (node 1) instead of the third.
  const moved = { ...duckOurs, nodes: [{ children: [1, 2] }, { mesh: 0, camera: 0, translation: [0, 0, 5] }, {}] };
  assert.equal(sceneCameras(moved)[0].name, 'node 1');
  assert.notEqual(sceneCameras(moved)[0].name, sceneCameras(duckFile)[0].name);
  const r = compareFrames(sceneCameras(duckFile), sceneCameras(moved), 'static');
  assert.equal(r.checks.length, 0, 'unpaired cameras are not compared here; compareScene fails each one (next)');
  const names = (cs) => cs.map((c) => nameCandidates(c.name));
  const got = sceneCameras(moved).map((c) => ({ ...c, name: names([c])[0][1] })); // node1, as FBX writes it
  const full = compareScene({ cameras: sceneCameras(duckFile), lights: [] }, { cameras: got, lights: [] }, 'fbx');
  assert.ok(full.failed >= 2, JSON.stringify(full.checks));
  assert.ok(full.checks.every((c) => c.field === 'present' && c.verdict === 'fail'));
});

test("an unnamed light pairs by rank too, and a named camera keeps its name", () => {
  const lit = {
    scene: 0,
    scenes: [{ nodes: [0, 3] }],
    nodes: [{ children: [2, 1] }, { extensions: { KHR_lights_punctual: { light: 0 } } }, { mesh: 0 }, { name: 'sun', extensions: { KHR_lights_punctual: { light: 0 } } }],
    extensions: { KHR_lights_punctual: { lights: [{ type: 'point' }] } },
  };
  // Depth-first: 0, 2, 1, 3, so the unnamed light on node 1 is the third.
  assert.deepEqual(sceneLights(lit).map((l) => l.name), ['node 2', 'sun']);
  const named = { ...duckFile, nodes: [{ children: [2, 1] }, { name: 'eye', camera: 0 }, { mesh: 0 }] };
  assert.equal(sceneCameras(named)[0].name, 'eye');
});

test("a named node pairs under each writer's sanitised form of its name", () => {
  // Collada `name` (dae.rs ncname: an NCName, so a space becomes _), Collada
  // id (dae.rs Ids::make: ASCII letters, digits, _ and -, so a dot becomes _
  // too; assimp names a Collada node after it), USD prim name (usd_write.rs
  // identifier: ASCII letters, digits and _).
  assert.deepEqual(nameCandidates('Generated Camera'), ['Generated Camera', 'Generated_Camera']);
  assert.deepEqual(nameCandidates('Light 4 - White'), ['Light 4 - White', 'Light_4_-_White', 'Light_4___White']);
  // SceneKit reads a Collada node's name with each `.` made `-` (measured).
  assert.deepEqual(nameCandidates('Point.002_Orientation'), ['Point.002_Orientation', 'Point_002_Orientation', 'Point-002_Orientation']);
  assert.deepEqual(nameCandidates('a.b.c'), ['a.b.c', 'a_b_c', 'a-b-c']);
  // A leading digit takes a _ in all three; a letter outside ASCII survives
  // in an NCName alone.
  assert.deepEqual(nameCandidates('3D Cam'), ['3D Cam', '_3D_Cam']);
  assert.deepEqual(nameCandidates('Lámpara 1'), ['Lámpara 1', 'Lámpara_1', 'L_mpara_1']);
  assert.deepEqual(nameCandidates('-x'), ['-x', '_-x', '_x']);
  // An id is cut at 64 characters, a prim name at 96.
  const long = 'a'.repeat(100);
  assert.deepEqual(nameCandidates(long), [long, 'a'.repeat(64), 'a'.repeat(96)]);
  // So assimp's reading of a Collada camera named by its sanitised id pairs.
  const aj = fbxCamera(ID3);
  aj.rootnode.children[0].children[0].name = 'Generated_Camera';
  aj.cameras[0] = { ...aj.cameras[0], name: 'Generated_Camera', horizontalfov: 1.13036, lookat: [0, 0, -1] };
  const r = compareScene({ cameras: [{ ...sceneCamera, name: 'Generated Camera' }], lights: [] }, { cameras: assimpCameras(aj, 'dae'), lights: [] }, 'dae');
  assert.equal(check(r, 'camera Generated Camera', 'eye').verdict, 'pass');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
});

test("a nameless Collada camera is named after its id for assimp, which skips its optics otherwise", () => {
  const dae = [
    '<library_cameras>',
    '    <camera id="camera0-camera">',
    '      <optics><technique_common><perspective><yfov>45.8</yfov></perspective></technique_common></optics>',
    '    </camera>',
    '    <camera id="persp_camera-camera" name="persp_camera">',
    '    </camera>',
    '</library_cameras>',
    '<node id="node3" type="NODE"><instance_camera url="#camera0-camera"/></node>',
  ].join('\n');
  const named = namedColladaCameras(dae);
  assert.ok(named.includes('<camera id="camera0-camera" name="camera0-camera">'), named);
  assert.ok(named.includes('<camera id="persp_camera-camera" name="persp_camera">'), 'a named camera is left alone');
  assert.ok(named.includes('<instance_camera url="#camera0-camera"/>'), 'an instance is left alone');
  assert.equal(named.replace(' name="camera0-camera"', ''), dae, 'nothing else changes');
  assert.equal(namedColladaCameras(named), named, 'a file with every camera named is unchanged');
});

// Followups 2026-11-08, B: a Collada <perspective> with <yfov> alone, which
// COLLADA 1.4.1 allows and litegen writes for a camera with no aspect ratio.
// assimp's ColladaLoader (v6.0.5 L418-L436) sets mHorizontalFOV from <xfov>,
// or from <yfov> with <aspect_ratio>, and otherwise leaves aiCamera's default
// of pi/4, which it then converts from degrees to radians: pi/4 * pi/180 =
// 0.0137078, whatever the file says.
const DAE_OPTICS = (persp) =>
  [
    '<library_cameras>',
    ...persp.map(([id, body]) => `  <camera id="${id}" name="${id}"><optics><technique_common><perspective>${body}<znear>1</znear><zfar>100</zfar></perspective></technique_common><extra><technique profile="litegen"><gltf_yfov>1</gltf_yfov></technique></extra></optics></camera>`),
    '  <camera id="o-camera"><optics><technique_common><orthographic><xmag>3</xmag><ymag>2</ymag></orthographic></technique_common></optics></camera>',
    '</library_cameras>',
    '<library_visual_scenes><visual_scene id="Scene">',
    '  <node id="rig" type="NODE">',
    '    <node id="cam_y" type="NODE"><instance_camera url="#y-camera"/></node>',
    '    <node id="cam_ya" type="NODE"><instance_camera url="#ya-camera"/></node>',
    '    <node id="cam_xy" type="NODE"/>',
    '  </node>',
    '  <node id="cam_x" type="NODE"><instance_camera url="#x-camera"/></node>',
    '  <node id="cam_o" type="NODE"><instance_camera url="#o-camera"/></node>',
    '  <node id="cam_missing" type="NODE"><instance_camera url="#nowhere"/></node>',
    '</visual_scene></library_visual_scenes>',
  ].join('\n');
const daeText = DAE_OPTICS([
  ['y-camera', '<yfov>57.29577951308232</yfov>'],
  ['ya-camera', '<yfov>45</yfov><aspect_ratio>1.5</aspect_ratio>'],
  ['x-camera', '<xfov>90</xfov>'],
]);

test('a Collada file says which of <xfov>, <yfov> and <aspect_ratio> each camera node has', () => {
  const o = colladaCameraOptics(daeText);
  assert.deepEqual([...o.keys()].sort(), ['cam_x', 'cam_y', 'cam_ya'], 'perspective cameras on nodes: not the orthographic one, nor a missing library entry');
  assert.deepEqual(o.get('cam_y'), { yfov: 57.29577951308232, xfov: undefined, aspectRatio: undefined });
  assert.deepEqual(o.get('cam_ya'), { yfov: 45, xfov: undefined, aspectRatio: 1.5 });
  assert.deepEqual(o.get('cam_x'), { yfov: undefined, xfov: 90, aspectRatio: undefined });
  assert.equal(colladaCameraOptics('').size, 0);
});

// assimp's reading of the lone <yfov> camera of VirtualCity, as seen in
// assjson: the default pi/4 turned from degrees to radians, aspect 0.
const loneYfov = (over = {}) => {
  const aj = fbxCamera(ID3);
  aj.cameras[0] = { ...aj.cameras[0], aspect: 0, horizontalfov: 0.0137078, lookat: [0, 0, -1], ...over };
  return assimpCameras(aj, 'dae').map((c) => ({ ...c, name: 'cam_y' }));
};
const withOptics = (cams, optics) => cams.map((c) => ({ ...c, colladaOptics: optics }));
const unsetAspect = { ...sceneCamera, name: 'cam_y', yfov: 1, aspect: undefined };

test("a Collada camera with <yfov> alone skips assimp's fov, which it cannot recover, and judges the file's own <yfov>", () => {
  const [g] = loneYfov();
  near(g.yfov, 0.0137078, 1e-6, "assimp's yfov is the default pi/4, in degrees mistaken for radians");
  const r = compareScene({ cameras: [unsetAspect], lights: [] }, { cameras: withOptics([g], colladaCameraOptics(daeText).get('cam_y')), lights: [] }, 'dae');
  assert.equal(r.failed, 0, JSON.stringify(r.checks.filter((c) => c.verdict === 'fail')));
  const skip = check(r, 'camera cam_y', 'yfov');
  assert.equal(skip.verdict, 'skip');
  assert.match(skip.reason, /ColladaLoader\.cpp L418-L436/);
  assert.match(skip.reason, /neither <xfov> nor <aspect_ratio>/);
  assert.equal(check(r, 'camera cam_y', 'aspect').verdict, 'skip', 'the scene camera has no aspect of its own');
  const own = check(r, 'camera cam_y', "yfov (the file's own <yfov>)");
  assert.equal(own.verdict, 'pass');
  near(own.got, 1, 1e-12, '57.29577951308232 degrees');
});

test("a lone <yfov> written wrong still fails, in the file's own number", () => {
  const [g] = loneYfov();
  const r = compareScene({ cameras: [unsetAspect], lights: [] }, { cameras: withOptics([g], { yfov: 60, xfov: undefined, aspectRatio: undefined }), lights: [] }, 'dae');
  assert.equal(check(r, 'camera cam_y', "yfov (the file's own <yfov>)").verdict, 'fail');
  assert.equal(r.failed, 1);
});

test('a lone <yfov> whose scene camera has an aspect ratio still fails: the aspect was lost', () => {
  const [g] = loneYfov();
  const r = compareScene({ cameras: [{ ...unsetAspect, aspect: 1.5 }], lights: [] }, { cameras: withOptics([g], { yfov: 57.29577951308232, xfov: undefined, aspectRatio: undefined }), lights: [] }, 'dae');
  assert.equal(check(r, 'camera cam_y', 'aspect').verdict, 'fail');
});

test('the skip is for <yfov> alone: a camera with <aspect_ratio> or <xfov> is judged on assimp\'s fov as before', () => {
  // <yfov> with <aspect_ratio>: assimp recovers it (L430-L432), so a wrong
  // reading fails; this camera's reading is the lone one's, wrongly.
  const [g] = loneYfov();
  for (const optics of [{ yfov: 57.29577951308232, xfov: undefined, aspectRatio: 1.5 }, { yfov: undefined, xfov: 90, aspectRatio: undefined }, undefined]) {
    const r = compareScene({ cameras: [unsetAspect], lights: [] }, { cameras: optics ? withOptics([g], optics) : [g], lights: [] }, 'dae');
    assert.equal(check(r, 'camera cam_y', 'yfov').verdict, 'fail', JSON.stringify(optics));
  }
  // Another importer's yfov is never skipped for this.
  const glAj = fbxCamera(ID3);
  glAj.cameras[0] = { ...glAj.cameras[0], horizontalfov: 0.0137078, lookat: [0, 0, -1] };
  const gl = assimpCameras(glAj, 'glb').map((c) => ({ ...c, name: 'cam_y', colladaOptics: { yfov: 57.3 } }));
  assert.equal(check(compareScene({ cameras: [unsetAspect], lights: [] }, { cameras: gl, lights: [] }, 'glb'), 'camera cam_y', 'yfov').verdict, 'fail');
});

test("a glTF output's orthographic magnifications are read from its own JSON", () => {
  const aj = fbxCamera(ID3);
  aj.cameras[0] = { ...aj.cameras[0], horizontalfov: 0, lookat: [0, 0, -1] };
  const json = { nodes: [{ name: 'persp_camera_node', camera: 0 }], cameras: [{ type: 'orthographic', orthographic: { xmag: 3, ymag: 2, znear: 0.125, zfar: 64 } }] };
  const ortho = { ...sceneCamera, projection: 'orthographic', yfov: undefined, xmag: 3, ymag: 2, aspect: 1.5 };
  const r = compareScene({ cameras: [ortho], lights: [] }, { cameras: withGltfMagnifications(assimpCameras(aj, 'glb'), json), lights: [] }, 'glb');
  assert.equal(check(r, 'camera persp_camera_node', 'xmag').verdict, 'pass');
  assert.equal(check(r, 'camera persp_camera_node', 'ymag').verdict, 'pass');
  json.cameras[0].orthographic.xmag = 4;
  const wrong = compareScene({ cameras: [ortho], lights: [] }, { cameras: withGltfMagnifications(assimpCameras(aj, 'glb'), json), lights: [] }, 'glb');
  assert.equal(check(wrong, 'camera persp_camera_node', 'xmag').verdict, 'fail');
});

// ─── the expected scene, read from its glTF JSON ────────────────────────────

// A GLB of a JSON chunk and a BIN chunk, as litegen writes one.
const glb = (json, bin) => {
  const pad = (b, fill) => Buffer.concat([b, Buffer.alloc((4 - (b.length % 4)) % 4, fill)]);
  const j = pad(Buffer.from(JSON.stringify(json)), 0x20);
  const b = pad(bin, 0);
  const u32 = (...xs) => Buffer.from(new Uint32Array(xs).buffer);
  return Buffer.concat([u32(0x46546c67, 2, 12 + 8 + j.length + 8 + b.length), u32(j.length, 0x4e4f534a), j, u32(b.length, 0x004e4942), b]);
};
// staged's rig and first camera, the rig turned 0, 45 and 90° about +Y at
// 0, 1 and 2 s (the USD stage above holds the same keys at 30 codes a
// second).
const animatedScene = (interpolation = 'LINEAR') => {
  const f = new Float32Array([0, 1, 2, 0, 0, 0, 1, 0, Math.sin(Math.PI / 8), 0, Math.cos(Math.PI / 8), 0, H, 0, H]);
  const json = {
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [
      { name: 'rig', translation: [0.5, 0, -0.25], children: [1] },
      { name: 'persp_camera_node', camera: 0, translation: [0, 1.5, 4], rotation: [-0.12218326, 0, 0, 0.9925076] },
    ],
    cameras: [{ type: 'perspective', perspective: { yfov: 0.8, aspectRatio: 1.5, znear: 0.125, zfar: 64 } }],
    buffers: [{ byteLength: f.byteLength }],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: 12 }, { buffer: 0, byteOffset: 12, byteLength: 48 }],
    accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: 'SCALAR' }, { bufferView: 1, componentType: 5126, count: 3, type: 'VEC4' }],
    animations: [{ name: 'Orbit', channels: [{ sampler: 0, target: { node: 0, path: 'rotation' } }], samplers: [{ input: 0, output: 1, interpolation }] }],
  };
  return { json, bin: Buffer.from(f.buffer) };
};

test("the scene's cameras at a time follow its first animation, as glTF 2.0 samples one", () => {
  const a = animatedScene();
  const { json, bin } = readGlb(glb(a.json, a.bin));
  assert.equal(json.animations[0].name, 'Orbit');
  const at = (t) => sceneCameras(json, t, bin)[0];
  nearVec(at(null).eye, [0.5, 1.5, 3.75], 1e-6, 'rest');
  nearVec(at(1).eye, [0.5 + 4 * H, 1.5, -0.25 + 4 * H], 1e-6, 't = 1 eye');
  nearVec(at(1).forward, turnY(45)(usdPersp.forward), 1e-6, 't = 1 forward');
  nearVec(at(1).up, turnY(45)(usdPersp.up), 1e-6, 't = 1 up');
  // LINEAR rotations slerp: 22.5° at 0.5 s; the last key holds past it.
  nearVec(at(0.5).eye, [0.5 + 4 * Math.sin(Math.PI / 8), 1.5, -0.25 + 4 * Math.cos(Math.PI / 8)], 1e-6, 't = 0.5');
  nearVec(at(5).eye, [4.5, 1.5, -0.25], 1e-6, 't = 5');
  // STEP holds each key until the next.
  json.animations[0].samplers[0].interpolation = 'STEP';
  nearVec(at(0.5).eye, [0.5, 1.5, 3.75], 1e-6, 'STEP at 0.5');
  nearVec(at(1.5).eye, [0.5 + 4 * H, 1.5, -0.25 + 4 * H], 1e-6, 'STEP at 1.5');
});

test("a CUBICSPLINE channel is a Hermite curve of its keys' tangents", () => {
  // The rig's translation from (0.5, 0, -0.25) at 0 s to (1.5, 0, -0.25) at
  // 2 s; each key is (in-tangent, value, out-tangent).
  const f = new Float32Array([0, 2, 0, 0, 0, 0.5, 0, -0.25, 0, 0, 0, 0, 0, 0, 1.5, 0, -0.25, 0, 0, 0]);
  const json = {
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ name: 'rig', translation: [0.5, 0, -0.25], children: [1] }, { name: 'cam', camera: 0 }],
    cameras: [{ type: 'perspective', perspective: { yfov: 0.8, znear: 0.125 } }],
    buffers: [{ byteLength: f.byteLength }],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: 8 }, { buffer: 0, byteOffset: 8, byteLength: 72 }],
    accessors: [{ bufferView: 0, componentType: 5126, count: 2, type: 'SCALAR' }, { bufferView: 1, componentType: 5126, count: 6, type: 'VEC3' }],
    animations: [{ channels: [{ sampler: 0, target: { node: 0, path: 'translation' } }], samplers: [{ input: 0, output: 1, interpolation: 'CUBICSPLINE' }] }],
  };
  const bin = Buffer.from(f.buffer);
  // Flat tangents: halfway in time is halfway along.
  nearVec(sceneCameras(json, 1, bin)[0].eye, [1, 0, -0.25], 1e-6, 'flat');
  // An out-tangent of (1, 0, 0) a second at the first key adds
  // h10(0.5)·dt·m0 = 0.125 · 2 · 1 = 0.25 along X.
  f[8] = 1;
  nearVec(sceneCameras(json, 1, Buffer.from(f.buffer))[0].eye, [1.25, 0, -0.25], 1e-6, 'tangent');
});

test('glTF cameras are placed by their nodes, translation, rotation and matrix', () => {
  const json = {
    scene: 0,
    scenes: [{ nodes: [0, 2] }],
    nodes: [
      { name: 'rig', translation: [0.5, 0, -0.25], children: [1] },
      { name: 'cam', camera: 0, translation: [0, 1.5, 4], rotation: [0, H, 0, H] },
      { name: 'flat', camera: 1, matrix: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 5, 6, 7, 1] },
    ],
    cameras: [
      { type: 'perspective', perspective: { yfov: 0.8, aspectRatio: 1.5, znear: 0.125, zfar: 64 } },
      { type: 'orthographic', orthographic: { xmag: 3, ymag: 2, znear: 0.0625, zfar: 20 } },
    ],
  };
  const [cam, flat] = sceneCameras(json);
  assert.equal(cam.name, 'cam');
  nearVec(cam.eye, [0.5, 1.5, 3.75], 1e-12, 'eye');
  nearVec(cam.forward, [-1, 0, 0], 1e-12, 'forward');
  nearVec(cam.up, [0, 1, 0], 1e-12, 'up');
  assert.deepEqual([cam.projection, cam.yfov, cam.aspect, cam.znear, cam.zfar], ['perspective', 0.8, 1.5, 0.125, 64]);
  nearVec(flat.eye, [5, 6, 7], 0, 'matrix eye');
  assert.deepEqual([flat.projection, flat.xmag, flat.ymag, flat.aspect], ['orthographic', 3, 2, 1.5]);
});

test('glTF lights fill in the extension defaults', () => {
  const json = {
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ name: 'lamp', extensions: { KHR_lights_punctual: { light: 0 } } }],
    extensions: { KHR_lights_punctual: { lights: [{ type: 'spot' }] } },
  };
  const [l] = sceneLights(json);
  assert.equal(l.name, 'lamp');
  assert.deepEqual([l.kind, l.color, l.intensity, l.inner, l.outer], ['spot', [1, 1, 1], 1, 0, Math.PI / 4]);
  nearVec(l.direction, [0, 0, -1], 0, 'direction');
});

// ─── Collada clips as SceneKit plays them (followups 2026-11-08, D) ─────────

// CesiumMilkTruck as litegen writes it: its animation and a node are both
// named Wheels, so the clip's id is the de-duplicated `Wheels-2` (dae.rs
// `Ids::make`) and its name `Wheels`.
const truckDae = [
  '<library_geometries><geometry id="Wheels-mesh" name="Wheels"/></library_geometries>',
  '<library_animations>',
  '  <animation id="Wheels-anim"><channel source="#Wheels-anim-sampler" target="Wheels/transform"/></animation>',
  '  <animation id="Wheels_001-anim"><channel source="#x" target="Wheels_001/transform"/></animation>',
  '</library_animations>',
  '<library_animation_clips>',
  '  <animation_clip id="Wheels-2" start="0" end="1.25" name="Wheels">',
  '    <instance_animation url="#Wheels-anim"/><instance_animation url="#Wheels_001-anim"/>',
  '  </animation_clip>',
  '</library_animation_clips>',
  '<library_visual_scenes><visual_scene id="Scene">',
  '  <node id="Wheels" name="Wheels" type="NODE"/><node id="Wheels_001" name="Wheels.001" type="NODE"/>',
  '</visual_scene></library_visual_scenes>',
].join('\n');
const foxDae = (clip) => truckDae.replace(/<animation_clip [^>]*>/, clip);

test("SceneKit knows a clip by its name, so a clip named like its node has no entry to play", () => {
  const { clips } = colladaClips(truckDae);
  assert.equal(clips.length, 1);
  assert.deepEqual([clips[0].id, clips[0].name, clips[0].key, clips[0].shadowed], ['Wheels-2', 'Wheels', 'Wheels', true]);
  const r = sceneKitClip(truckDae, 'Wheels');
  assert.equal(r.clip, 'Wheels', "the key SceneKit would register it under, not the writer's id Wheels-2");
  assert.equal(r.notPlayed, SCENEKIT_CLIP_SHADOWED);
  assert.match(r.notPlayed, /measured 2026-11-08/);
});

test('a clip with a free key is played by that key: its name, else its id', () => {
  // Measured: id Wheels-2, name WheelsClip is entry WheelsClip; no name is the id.
  for (const [clip, key] of [
    ['<animation_clip id="Wheels-2" start="0" end="1.25" name="WheelsClip">', 'WheelsClip'],
    ['<animation_clip id="Zed-2" start="0" end="1.25">', 'Zed-2'],
    ['<animation_clip id="Walk_001" start="0" end="1.25" name="Walk.001">', 'Walk.001'],
    ['<animation_clip id="Wheels-2" start="0" end="1.25" name="Scene">', 'Scene'],
    ['<animation_clip id="Wheels-2" start="0" end="1.25" name="Wheels.001">', 'Wheels.001'],
  ]) {
    const r = sceneKitClip(foxDae(clip), key);
    assert.equal(r.clip, key, clip);
    assert.equal(r.notPlayed, null, clip);
  }
  // A key that is a geometry's, an animation's or another node's id is lost.
  for (const name of ['Wheels-mesh', 'Wheels-anim', 'Wheels_001']) {
    assert.equal(sceneKitClip(foxDae(`<animation_clip id="Wheels-2" start="0" end="1.25" name="${name}">`), name).notPlayed, SCENEKIT_CLIP_SHADOWED, name);
  }
});

test('clips that animate the same node still replace each other in SceneKit; a file with no clip plays none', () => {
  const two = truckDae.replace('</library_animation_clips>', '<animation_clip id="B" name="B"><instance_animation url="#Wheels-anim"/></animation_clip></library_animation_clips>').replace(/(<animation_clip id="Wheels-2"[^>]*)name="Wheels"/, '$1name="A"');
  const r = sceneKitClip(two, 'B');
  assert.equal(r.clip, 'B');
  assert.equal(r.notPlayed, SCENEKIT_SHARED_CLIPS);
  assert.deepEqual(sceneKitClip('<COLLADA/>', 'x'), { clip: null, notPlayed: null });
});

// ─── a camera with no aspect ratio, in usdrecord's picture (followups 2026-11-08, C) ───

test('a camera picture is SIZE high and SIZE x aspect wide, a square for no aspect', () => {
  assert.deepEqual(cameraViewFor(1.5, 256), { width: 384, height: 256 });
  assert.deepEqual(cameraViewFor(undefined, 256), { width: 256, height: 256 });
  assert.deepEqual(cameraViewFor(NaN, 256), { width: 256, height: 256 });
  assert.deepEqual(cameraViewFor(100, 256), { width: 1024, height: 256 }, 'at most four times as wide');
  assert.deepEqual(cameraViewFor(UNSET_ASPECT, 256), { width: 455, height: 256 }, "the 16:9 litegen writes for a camera with no aspect ratio");
});

test("usdrecord's picture of a camera with no aspect ratio is the expected one drawn 16:9, not square", () => {
  // VirtualCity's camera has no aspectRatio: litegen writes 0.4266667 /
  // 0.24 apertures (16:9), usdrecord draws --imageWidth 256 at 256x143 and, at
  // the expected 16:9 view (455 wide), 455x255 (frameRecorder.cpp truncation).
  const square = cameraViewFor(undefined, 256);
  const wide = cameraViewFor(UNSET_ASPECT, 256);
  // Before: compared with the square expected picture, the case failed.
  assert.deepEqual(externalPictureSize({ width: 256, height: 143 }, square), { error: 'the picture is 256x143, not 256x256' });
  // After: compared with the picture drawn at 16:9, the one lost row is the
  // documented truncation.
  const r = externalPictureSize({ width: 455, height: 255 }, wide);
  assert.deepEqual(Object.keys(r), ['rescaled']);
  assert.match(r.rescaled, /frameRecorder\.cpp L386-L391/);
  assert.deepEqual(externalPictureSize({ width: 455, height: 256 }, wide), {});
});

test('a camera whose aspect really changed still fails the size check', () => {
  const wide = cameraViewFor(UNSET_ASPECT, 256);
  // Negative control: the same camera written at 4:3 (usdrecord 455x341) or
  // at 1:1 (455x455), or at the 16:9 for a camera whose aspect is 1.5.
  assert.match(externalPictureSize({ width: 455, height: 341 }, wide).error, /^the picture is 455x341, not 455x256$/);
  assert.match(externalPictureSize({ width: 455, height: 455 }, wide).error, /455x455/);
  assert.match(externalPictureSize({ width: 455, height: 253 }, wide).error, /455x253/, 'two rows short is no truncation');
  assert.match(externalPictureSize({ width: 454, height: 255 }, wide).error, /454x255/, 'the width is exact');
  const set = cameraViewFor(1.5, 256);
  assert.match(externalPictureSize({ width: 384, height: 216 }, set).error, /384x216, not 384x256/, "an explicit 1.5 written 16:9 fails, though 16:9 is the unset camera's");
  assert.deepEqual(externalPictureSize({ width: 384, height: 256 }, set), {});
});

// ─── USD texture tint (followups 2026-11-08, E) ─────────────────────────────

// A flattened stage (`usdcat --flatten`) of materials, each a PreviewSurface and
// textures: `base` is the scale of the texture on `inputs:diffuseColor`, `normal`
// the scale of the one on `inputs:normal` (litegen writes (2, 2, 2, 1) there).
function usdStage(materials) {
  const tex = (path, name, scale) =>
    `            def Shader "${name}"\n            {\n                uniform token info:id = "UsdUVTexture"\n                asset inputs:file = @/tmp/a {b}.png@\n${scale ? `                float4 inputs:scale = ${scale}\n` : ''}                float3 outputs:rgb\n            }\n`;
  const body = materials
    .map(({ base, normal }, i) => {
      const p = `/Root/Materials/Material_${i}`;
      return `        def Material "Material_${i}"\n        {\n            def Shader "PreviewSurface"\n            {\n                uniform token info:id = "UsdPreviewSurface"\n                color3f inputs:diffuseColor.connect = <${p}/BaseColorTex.outputs:rgb>\n                normal3f inputs:normal.connect = <${p}/NormalTex.outputs:rgb>\n            }\n\n${tex(p, 'BaseColorTex', base)}\n${tex(p, 'NormalTex', normal)}        }\n`;
    })
    .join('\n');
  return `#usda 1.0\n(\n    doc = """Generated from { a flattened stage }\n"""\n    customLayerData = {\n        string creator = "x"\n    }\n)\n\ndef Xform "Root"\n{\n    def Scope "Materials"\n    {\n${body}    }\n}\n`;
}

test("a scale on the texture that feeds the base colour is a tint SceneKit ignores", () => {
  assert.equal(usdTextureTint(usdStage([{ base: '(1, 0, 0, 1)' }])), true, 'MorphPrimitivesTest red');
  assert.equal(usdTextureTint(usdStage([{ base: '(0, 1, 0, 1)' }])), true, 'MorphPrimitivesTest green');
  assert.equal(usdTextureTint(usdStage([{ base: '(0.5, 1, 1, 1)' }])), true);
  assert.equal(usdTextureTint(usdStage([{ base: '(1, 1, 1, 1)' }])), false, 'the default is no tint');
  assert.equal(usdTextureTint(usdStage([{}])), false, 'no scale at all');
  assert.equal(usdTextureTint(usdStage([{ base: '(1, 1, 1, 0.5)' }])), false, 'an alpha scale does not tint the opaque albedo');
  assert.equal(usdTextureTint(usdStage([{}, { base: '(1, 0, 0, 1)' }, {}])), true, 'one tinted material of three');
  assert.equal(usdTextureTint(''), false);
  assert.equal(usdTextureTint('float2 inputs:scale = (2, 2)\nfloat4 inputs:bias = (0.5, 0, 0, 0)'), false, 'a UsdTransform2d scale is not a tint, and bias is not measured');
  assert.match(SCENEKIT_USD_TEXSCALE, /shaderDefs\.usda L196-L202/);
});

test("a normal map's (2, 2, 2, 1) scale is not a tint: it feeds no base colour, and SceneKit's albedo of it is judged", () => {
  // Khronos LightsPunctualLamp: every material's NormalTex has scale (2, 2, 2, 1) and bias (-1, -1, -1, 0)
  // and SceneKit's albedo of the file is 0 % apart from the expected picture.
  assert.equal(usdTextureTint(usdStage([{ normal: '(2, 2, 2, 1)' }, { normal: '(2, 2, 2, 1)' }])), false);
  assert.equal(usdTextureTint(usdStage([{ normal: '(2, 2, 2, 1)' }, { base: '(1, 0, 0, 1)', normal: '(2, 2, 2, 1)' }])), true, 'a tinted base colour beside a normal map still counts');
});
