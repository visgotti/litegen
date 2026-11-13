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
// written as GLB and rendered by GLTFLoader. The material layers (clearcoat,
// sheen, transmission, ...) show in the lit modes, `lit` and `lit_coat`
// (page/render.js), asked for where the expected file keeps a layer the
// judge can see and the target's loader can build one (litPlan). glTF outputs are additionally run through the
// Khronos glTF-Validator, USD through Apple's usdchecker (with `--arkit` on
// USDZ alone), and every format assimp can import through assimp, so a file
// that renders but is invalid still fails. A glTF output of a glTF original
// (a matrix made with `--import`) must also keep the original's layer
// extensions, field by field (layer-extensions.mjs). A file's own lights and
// cameras (mesh-gaps item 5) are judged in the `camera` and `lit_lights`
// modes and by the numeric oracle, camlight-oracle.mjs (CAMLIGHT below).
//
// Exit status is non-zero when any case fails.
//
// Ruling IDs ("Ruling A35") are mesh-gaps item 4's rulings, listed with
// their costs in the status block of
// docs/superpowers/plans/2026-11-04-mesh-material-layers.md.

import crypto from 'node:crypto';
import http from 'node:http';
import fs from 'node:fs/promises';
import fss from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import validator from 'gltf-validator';
import { compareFiles, usePixelDigest } from './layer-extensions.mjs';
import { loadPixelDigest, loadPixelReader } from './pixel-digest.mjs';
import { sceneKitClip, SCENEKIT_USD_TEXSCALE, usdTextureTint } from './scenekit-limits.mjs';
import {
  assimpKeepVerdict,
  ASSIMP_KEEP_CITE,
  compareNearPixels,
  DAE_KEEP,
  daeKeepSummary,
  hasMorph,
  keepMissingMorph,
  keepSilhouetteVerdict,
  keepUvJudge,
  prepareSchema,
  reportedTools,
  schemaForReport,
  threeKeepVerdict,
  xmllintFile,
} from './dae-keep.mjs';
import { assimpView, nameCandidates, oracleFile, readGltfJson, sceneLights, srgbDecode, TOLERANCE as ORACLE_TOLERANCE, usdCameraFile, usdPrims } from './camlight-oracle.mjs';
import {
  animationVariants,
  baseColourMatch,
  clipRanges,
  clipVariantsVerdict,
  crateEmptyArrays,
  crateRooted,
  clipSampleTimes,
  groupNames,
  lightInvariance,
  lightResponse,
  meshesOf,
  missingVariantsReason,
  pixelsDiffer,
  pureMasks,
  textDiff,
  threeClipsVerdict,
  UNLIT_LIMITS,
  unlitVerdict,
  usdcatVerdict,
  usdcheckerResults,
  usdcheckerVariantVerdict,
  usdVariantCase,
  variantOver,
} from './usd-variants.mjs';

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
// A `dae-keep` case (mesh_matrix --dae-keep, mesh-gaps item 8) is a Collada file
// for every judge that reads `dae`: the loop over the manifest's cases gives it
// `to: 'dae'` and `variant: 'dae-keep'`, and its pictures and files go under
// the name `<from>__dae-keep`, beside the plain case's `<from>__dae`. What
// differs is dae-keep.mjs's: three.js and assimp are expected to refuse a file
// that holds a `<morph>`, and xmllint judges the file against the COLLADA 1.4.1
// schema.
const caseId = (c) => `${c.from}__${c.variant ?? c.to}`;
if (!args[0]) {
  console.error('usage: node verify.mjs <matrix_dir> <report_dir> [--fixtures a,b] [--formats glb,obj]');
  process.exit(2);
}

// Fraction of covered pixels allowed to differ, per mode. Rasterising the same
// triangles through different loaders is pixel-identical; what is allowed for
// is texture filtering at mip boundaries and 8-bit colour-space round trips.
// The set selector's `uv1`–`uv3` are the uv mode through another set.
// `lit` and `lit_coat` are 0: the lit picture of a glTF output comes from
// the same loader, renderer and environment as the expected picture, so it
// is bit-exact, and any pixel over the lit modes' own 1/255 tolerance
// (render.js, LIT_TOLERANCE) is a lost or changed layer.
// `camera` is the geom mode through the file's own camera, at geom's
// threshold; `lit_lights` the lit modes' 0 at their 1/255 tolerance (CAMLIGHT).
const THRESHOLD = { geom: 0.01, normals: 0.02, albedo: 0.015, uv: 0.02, uv1: 0.02, uv2: 0.02, uv3: 0.02, lit: 0, lit_coat: 0, camera: 0.01, lit_lights: 0 };

// The native albedo threshold of a KTX2 original (mesh-gaps item 3): the
// original drawn by GLTFLoader with KTX2Loader against our reading of it, which
// is PNG (`mesh_matrix --transcode-textures`, the production path). KTX2Loader
// transcodes to the GPU format the browser reports (render.js, ktx2Loader), so
// the original is not the exact RGBA8 our reading holds, and its mip levels are
// its encoder's where ours are the GPU's. Calibrated 2026-11-08 on this Mac's
// Chromium (ASTC for UASTC, ETC2 for ETC1S) over the six KTX2 originals of the
// README's "KTX2 originals": 0.0000 to 9.5445 % of pixels past tolerance 20
// (Khronos AnisotropyBarnLamp the worst, StainedGlassLamp 3.30 %, FlightHelmet
// 0.46 %), so 10 %, the measured maximum rounded up to a whole percent.
const KTX2_NATIVE_ALBEDO = 0.1;
// The same comparison with both sides sampled from level 0 alone (render.js,
// baseLevelOnly), where the content and the GPU format are all that can differ:
// 0 pixels past tolerance 20 on all six originals on this machine. 0.1 % leaves
// room for another GPU's format (not measured: this machine has ETC2 and ASTC),
// and the negative control's smaller fault, 0.824 % (ChronographWatch, an ETC1S
// selector's bit order reversed), still fails it; its larger one was 2.217 %.
const KTX2_NATIVE_LEVEL0 = 0.001;
// The same pair through KTX2Loader's RGBA32 fallback (render.js, ktx2Rgba32),
// where both sides are the transcoder's own RGBA8: judged at no pixel past this
// channel difference (0-255), and at ratio 0. Litegen's decoder equals that
// output byte for byte (the unit test `every_reference_texture_transcodes_bit_
// exactly`), so the measured maximum of the six good runs is recorded in the
// report (`ktx2.rgba32.maxDelta`) and the README's calibration table.
const KTX2_RGBA32_TOLERANCE = 0;

// ─── lit: the material layers ───────────────────────────────────────────────
//
// The `lit` mode (page/render.js) draws the loader's own material under the
// RoomEnvironment PMREM, and only a loader that builds a MeshPhysicalMaterial
// can show a layer. Which reader judges a case's layers in `lit`:
//
//  - three.js GLTFLoader, for glb and gltf: every KHR_materials_* layer the
//    expected file shows (measured per file, layerVisibility; dispersion
//    shows nowhere), at 0.00 % with a per-channel tolerance of 1
//    (render.js, LIT_TOLERANCE; the same loader, renderer and data as the
//    expected picture; calibrated 2026-11-05 on `layered`: the expected file
//    against itself and every glTF case 0 pixels apart). The README's
//    per-layer table records what removing each layer changes.
//  - three.js USDLoader, for usdz and usda: it builds a MeshPhysicalMaterial
//    and reads `inputs:clearcoat`, `inputs:clearcoatRoughness` and
//    `inputs:ior` as values (USDComposer.js:2661-2671, :2626-2630), but
//    decodes a base colour as sRGB and follows no coat texture. So its judge
//    is `lit_coat`: `lit` with only the meshes it reads right drawn, on both
//    sides (render.js, usdLitJudgeable: unit base and emissive colours, no
//    vertex colours, an untextured coat), at 0.00 %, where the expected
//    picture has such a mesh; the whole-file `lit` stays a cited skip with
//    its picture (render.js, usdLimitation).
//  - usdrecord with a dome light, for usdz and usda: NOT a judge
//    (USDRECORD_LIT_NOT_A_JUDGE); its pictures stay in the report.
//  - nothing, for obj, dae, fbx, 3mf, ply and stl: their loaders build Phong
//    or Lambert materials and read no layer property (LIT_SKIP, each citing
//    the loader's lines), so the mode is not drawn for them.
//
// A case whose expected scene keeps no layer skips the mode: there is nothing
// to judge, and every layer-free material is already judged unlit.
const LIT_SKIP = {
  obj: 'three.js MTLLoader builds a MeshPhongMaterial (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/MTLLoader.js:511) and its property switch (:387-507) has no case for Pc, Pcr, Ps, Ni, aniso or anisor, so no layer reaches a picture; OBJ layers are judged by the obj -> glb cases',
  dae: 'three.js ColladaLoader builds a MeshPhongMaterial or MeshLambertMaterial (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/collada/ColladaComposer.js:1170, :1174) and parseEffectParameters reads emission, diffuse, specular, bump, ambient, shininess, transparency and transparent alone (collada/ColladaParser.js:751-767), never <index_of_refraction>; Collada keeps ior alone, and the iors a dae -> glb case carries (glass 1.52, plastic 1.6; 1.45 is a Collada default and dropped) move at most 1/255, so no lit case judges them (dae -> glb is a measured skip)',
  fbx: 'three.js FBXLoader builds a MeshPhongMaterial or MeshLambertMaterial (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/FBXLoader.js:536-547), which hold no layer',
  '3mf': 'three.js ThreeMFLoader builds a MeshStandardMaterial (roughness and metalness alone) or a MeshPhongMaterial (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/3MFLoader.js:1404, :1410), which hold no layer',
  ply: 'three.js PLYLoader returns a bare BufferGeometry and no material (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/PLYLoader.js:776)',
  stl: 'three.js STLLoader returns a bare BufferGeometry and no material (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/STLLoader.js:279, :374)',
};
const LIT_NO_LAYER = 'the expected scene keeps no material layer; nothing to judge lit, and the material is judged unlit';
// usdrecord under a constant-colour dome light is not a judge of a coat
// (Ruling A32). Storm adds the coat lobe on top of the base
// (/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:330-344,
// Apple USD Tools 0.25.2; F0 from the ior, :296) where three.js weights the
// base by the coat's Fresnel, so Storm's and three.js's pictures of a CORRECT
// file differ on 39-60 % of pixels against `lit_flat` (the coat cases on
// 55-60 %), and under a uniform dome a coat barely moves Storm's own
// picture: control (a) (the lacquer's coat written as 0.2 instead of 1,
// car_paint's coat map lost) moves the lacquer by at most
// USDRECORD_A32.maxDelta/255, no pixel past the 20/255 tolerance (only
// where a usd -> usd case lost car_paint's coat map does Storm move 434
// pixels by up to 44/255), and the ratio by 2.1-6.5 points. Measured
// 2026-11-05 on the final `layered` (USDRECORD_A32, harness/usdlit-diff.mjs):
// the correct coat cases' worst ratio against the control's best leaves a
// margin of about two points on a ~60 % baseline, made of pixels sitting at
// the tolerance edge, not of a coat Storm shows. Too fragile to judge; the
// picture, its diff and its ratio stay in the report.
const USDRECORD_A32 = { correct: '55.47-59.89 %', control: '61.75-62.01 %', maxDelta: 10 };
const USDRECORD_LIT_NOT_A_JUDGE = `usdrecord's dome-lit render is not a judge of a coat (Ruling A32): Storm adds the coat lobe on top of the base (/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:330-344, Apple USD Tools 0.25.2; F0 from the ior, :296) where three.js weights the base by the coat's Fresnel, so a correct coated file differs from the flat-lit expected picture on ${USDRECORD_A32.correct} of pixels, and under a uniform dome a coat barely moves Storm's picture: control (a) moves the coat mesh by at most ${USDRECORD_A32.maxDelta}/255 (no pixel past 20/255) and the ratio to ${USDRECORD_A32.control}, a margin of about two points made of pixels at the tolerance edge; USD coats are judged by lit_coat and the usd -> glb cases`;
const USDRECORD_DOME_INTENSITY = 0.35;
const USD_LIT_COAT_NONE =
  'no mesh of the expected scene that three.js USDLoader shows lit (a base and emissive colour of 0 or 1 per channel, no vertex colours, a coat without maps: it decodes colours as sRGB, scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:2512-2520 and :2541-2562, keeps displayColor[0], :1089-1096, and reads the coat as values, :2661-2671) carries a layer the lit judge can see; the layers this file keeps are judged by the usd -> glb cases where the table of what lit sees allows (a dae -> usd file keeps only iors that move at most 1/255, which nothing judges in lit)';
// What a lit judge can see is measured per expected file (page/render.js,
// layerVisibility: each layer removed in turn, the variant rendered, a layer
// seen when a pixel moves past the lit tolerance). A case is judged in a
// mode only where one of its layers is seen there; a case whose layers all
// move nothing is a measured skip (dispersion moves nothing anywhere; an
// uncoated glass's ior of 1.52 moves at most 1/255). `report.json` carries
// each expected file's measurement as `litSeen`, and each judged case's
// unseen layers as `litUnseen`.
const litVisibility = new Map();
const describeUnseen = (layers, vis, mode) => layers.map((l) => `${l} (max delta ${vis[l]?.[mode]?.maxDelta ?? 'unmeasured'}/255)`).join(', ');
const litUnseenReason = (layers, vis, mode) =>
  `measured: none of the expected scene's layer(s), ${describeUnseen(layers, vis, mode)}, moves a pixel past the lit tolerance when removed from the expected file and rendered through GLTFLoader in ${mode} (page/render.js, layerVisibility), so the lit judge cannot see them here; where another expected scene shows the same layer (an ior through the lacquer's black base), that scene's cases judge it, and where none does, nothing judges it in lit`;
// Which lit modes three.js draws for a case (`three`), the lit modes it does
// not, with why (`skips`), and the layers the judge cannot see in a judged
// case (`unseen`), from the expected picture's stats and visibility.
function litPlan(c, info, vis = {}) {
  if (!info?.layers?.length) return { three: [], skips: { lit: LIT_NO_LAYER } };
  if (LIT_SKIP[c.to]) return { three: [], skips: { lit: LIT_SKIP[c.to] } };
  // Fail closed: a layer without a measurement is neither judged nor
  // skipped; the case fails until the measurement exists.
  const unmeasured = info.layers.filter((l) => !vis[l]);
  if (unmeasured.length) {
    return { three: [], skips: {}, fail: `lit: no visibility measurement for layer(s) ${unmeasured.join(', ')} of expected ${c.expected} (page/render.js, layerVisibility measures each KHR_materials_* extension of the expected file; a layer it did not measure cannot be judged or skipped)` };
  }
  const seen = (layers, mode) => layers.filter((l) => vis[l]?.[mode]?.seen);
  const unseen = (layers, mode) => layers.filter((l) => !vis[l]?.[mode]?.seen);
  if (USD_FORMATS[c.to]) {
    // `lit` is drawn and kept (render.js skips it with its reason); the judge
    // is `lit_coat`, where a mesh USDLoader reads right carries a layer the
    // judge can see there.
    const coat = info.litCoatLayers || [];
    if (seen(coat, 'lit_coat').length) return { three: ['lit', 'lit_coat'], skips: {}, unseen: unseen(coat, 'lit_coat') };
    return { three: ['lit'], skips: { lit_coat: coat.length ? litUnseenReason(coat, vis, 'lit_coat') : USD_LIT_COAT_NONE } };
  }
  if (!seen(info.layers, 'lit').length) return { three: [], skips: { lit: litUnseenReason(info.layers, vis, 'lit') } };
  return { three: ['lit'], skips: {}, unseen: unseen(info.layers, 'lit') };
}

// The native check's lit pass (Ruling A35): a glTF original whose layers (as
// GLTFLoader builds them, on either side) are not none is drawn in `lit` by
// its own loader and our reading of it through one camera, the fixture's
// (page/render.js, renderNativeLit), judged at THRESHOLD.lit with the lit
// tolerance, and the two sides' layer sets must be equal (a layer `lit`
// cannot see, dispersion, still has to come through). Measured on the 16
// Khronos originals of task 5b: 15 are 0 pixels apart; ToyCar is 1 pixel
// past the tolerance, and 0 with its own rotations put back into our reading
// — the one difference this excuses, and only where the repeat closes it.
const GLTF_NATIVE = { glb: true, gltf: true };
// The excuse holds only for what a renormalization moves: every restored
// quaternion within this of ours, component by component (q and -q being one
// rotation). ToyCar's eleven moved by 4.85e-8 at most; a turn of 1° moves one
// by about 0.0087, and fails.
const LIT_ROTATION_BOUND = 1e-6;
const nativeLitRotations = (lit, restored) =>
  `litegen's glTF reader renormalizes a node rotation (litegen-core/src/mesh/glb.rs:1181-1190) and its GLB writer writes it in f64 (glb.rs:3170-3171, math::quat_normalize), where three.js composes the original's f32-rounded, slightly non-unit quaternion as given, so our reading is turned by about 1e-7: ${lit.differ} of ${lit.covered} lit pixels past the tolerance (max delta ${lit.maxDelta}), and ${restored.differ} with the original's ${restored.rotations} rotation(s) put back into our reading, none moved by more than ${restored.largest} (bound ${LIT_ROTATION_BOUND}; page/render.js, renderNativeLit; Khronos ToyCar in task 5b)`;
// SceneKit and usdrecord draw alpha, the expected pictures do not (Ruling
// A37): the expected albedo picture is drawn with an opaque MeshBasicMaterial
// (page/render.js, albedoMaterial) and its coverage is that picture's, while
// SceneKit and Storm draw a translucent material and a texture's alpha. So
// where the expected file has any (page/render.js, stats.translucent,
// stats.textureAlpha and stats.vertexAlpha), those two oracles' albedo and
// silhouette are skipped, their pictures kept. Measured in task 5b: Khronos TransmissionTest's
// Collada through SceneKit 3.38 % of pixels apart, 0.00 % with its textures'
// alpha removed; GlassVaseFlowers 39.27 %, 0.06 % made opaque and without
// texture alpha; IridescenceMetallicSpheres' silhouette 4.52 %, 0.27 %
// without its <transparent>, its albedo 7.70 %, 0.38 % without texture alpha;
// GlassVaseFlowers ply -> dae (the glass's alpha baked into a PLY's vertex
// colours, read back with no material) 34.82 %, 0.00 % with that alpha at 1.
const ALPHA_ORACLE_SKIP =
  'the expected scene has a translucent material, a base-colour texture with alpha or vertex colours with alpha, which the expected albedo and coverage pictures draw as one opaque colour (page/render.js, albedoMaterial) and SceneKit and usdrecord draw blended (measured in task 5b, Ruling A37: Khronos TransmissionTest through SceneKit 3.38 % of pixels apart, 0.00 % with its textures\' alpha removed; GlassVaseFlowers 39.27 %, 0.06 % opaque and without texture alpha; IridescenceMetallicSpheres\' silhouette 4.52 %, 0.27 % without its transparency; GlassVaseFlowers ply -> dae, the glass\'s alpha baked into vertex colours, 34.82 %, 0.00 % with that alpha at 1); three.js judges these colours in albedo, and their layers in lit';
// The layer-extension check (Ruling A20; README, "The layer-extension
// check"). `lit` cannot see the glTF writer drop a layer, since the expected
// picture comes from that writer too. So every glTF output of a glTF
// original, written from our reading of it (`scene`, `single`) or through
// glb or gltf, is compared with the original, layer extension by layer
// extension and field by field, floats as f32 (Ruling A20a), and any
// difference fails the case. Through the other containers a layer is lost by
// design: those losses are reported, and `lit` judges what they keep.
const LAYER_SOURCES = new Set(['scene', 'single', 'glb', 'gltf']);
const layerChecks = { cases: 0, failed: 0, f32Rounded: 0, ktx2Pixels: 0 };
// A KTX2 image of an original is written as PNG (mesh-gaps item 3): the layer
// check then compares the two images' decoded pixels, by readers that share
// nothing with litegen (pixel-digest.mjs: three's Binomial transcoder, and a
// PNG reader of its own). Without the transcoder (no `npm ci`) a KTX2 image
// stays the byte difference it looks like.
try {
  usePixelDigest(await loadPixelDigest());
} catch (e) {
  console.warn(`no pixel digest for KTX2 images: ${String(e?.message || e).slice(0, 200)}`);
}
function layerExtensionCheck(entry, source, output) {
  let r;
  try {
    r = compareFiles(source, output);
  } catch (e) {
    r = { verdict: 'fail', error: String(e?.message || e), diffs: [], f32Rounded: [], ktx2Pixels: [], unpaired: { src: [], out: [] } };
  }
  layerChecks.cases++;
  layerChecks.f32Rounded += r.f32Rounded.length;
  layerChecks.ktx2Pixels += r.ktx2Pixels.length;
  entry.layerExtensions = {
    verdict: r.verdict,
    ...(r.error ? { error: r.error } : { pairing: r.pairing, pairs: r.materials.length, excluded: r.excluded, otherExtensions: r.otherExtensions }),
    diffs: r.diffs,
    f32Rounded: r.f32Rounded,
    ...(r.ktx2Pixels.length ? { ktx2Pixels: r.ktx2Pixels } : {}),
    unpaired: r.unpaired,
  };
  if (r.verdict === 'pass') return;
  layerChecks.failed++;
  const show = (v) => (typeof v === 'number' ? String(v) : JSON.stringify(v));
  const said = [
    ...(r.error ? [r.error] : []),
    ...r.diffs.slice(0, 3).map((d) => `${d.material} / ${d.layer} / ${d.field}: ${show(d.src)} -> ${show(d.out)} [${d.kind}]`),
    ...(r.unpaired.src.length || r.unpaired.out.length ? [`unpaired source [${r.unpaired.src}], output [${r.unpaired.out}]`] : []),
  ];
  entry.reasons.push(`layer extensions differ from the original's (${r.diffs.length} difference(s)): ${said.join('; ')}`);
}

// A USD case ends with a judge of its geometry: three.js's geom or normals,
// usdrecord's cover or SceneKit's silhouette, in some frame. Each can be
// skipped for a cited reason (three.js reads no BlendShapes; Ruling A37's
// alpha gate skips the other two for a translucent expected scene). No input
// meets all three today, and one that did would pass with its geometry
// unjudged, so it fails.
const USD_NO_GEOMETRY_JUDGE =
  'no judge of its geometry: three.js geom and normals, usdrecord cover and SceneKit silhouette were each skipped, in every frame, or did not run';
const judgedIn = (byFrame) => !!byFrame && !byFrame.skipped && Object.values(byFrame).some((x) => x && typeof x === 'object' && !x.skipped && !x.error);
const geometryJudged = (entry) => ['geom', 'normals', 'usdrecord:cover'].some((m) => judgedIn(entry.results[m])) || judgedIn(entry.scenekit?.silhouette);
// SceneKit's albedo is judged on 4x4 block means (Ruling A38; page/render.js,
// compareImageBlocks), at the albedo tolerance, against this share of blocks;
// the per-pixel figure stays in the report as `pixelRatio`.
const SCENEKIT_ALBEDO_BLOCK_LIMIT = 0.02;
// Cited lit_coat skips, each with its measurement and the most pixels it
// covers (Ruling A39): beyond `maxDiffer` the case fails as usual.
const LIT_COAT_MEASURED = {
  'tp_ToyCar_glb|single|usda': {
    maxDiffer: 3,
    reason:
      "measured in task 5b (Ruling A39): GLTFLoader and USDLoader draw this coat, ToyCar's body (untextured in a single-file USDA, clearcoat 1 at roughness 0, lit_coat's exposure 4), 3 of its 2,068 pixels apart (max delta 144) where the data agree: with the source's rotations in the expected file the two loaders' geom and normals are bit-exact and lit_coat still differs on 2 pixels; a roughness-0 coat mirrors the room's area-light edges, and lit_coat's threshold 0 was calibrated on a flat box; skipped while the difference stays within 3 pixels",
  },
};

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
const USD_FORMATS = { usdz: true, usda: true, usdc: true };
// The prims Storm draws as lines: what litegen writes (BasisCurves) and the
// other curve types a third-party stage may hold (`Points` are the points).
const USD_CURVE_PRIMS = /^(BasisCurves|NurbsCurves|HermiteCurves)$/;

// ─── lights and cameras (mesh-gaps item 5) ──────────────────────────────────
//
// CAMLIGHT. A file's own lights and cameras (spec "Render harness"; item 5's
// rulings, R1–R32, are listed with their costs in the "Rulings" section of
// docs/superpowers/plans/2026-11-04-mesh-lights-cameras.md):
//
//  - `camera` (page/render.js): the geom mode through the file's own first
//    camera, at rest and at the sample times, drawn at the expected first
//    camera's aspect, against the expected GLB through its first camera.
//    Judges: three.js GLTFLoader for glb and gltf; assimp's reading for fbx,
//    the expected geometry drawn through a camera built from assimp's assjson
//    (Ruling R11: assimp 6.0's glTF2 exporter writes no camera); SceneKit's
//    silhouette through the file's camera for dae, usdz and usda; usdrecord
//    --camera for usdz and usda.
//  - `lit_lights`: the file's own material lit by its own lights alone,
//    through the fixture's camera. Judges: GLTFLoader for glb and gltf, and
//    assimp's reading for fbx (the expected GLB lit by lights built from it),
//    at 0.00 % with the lit tolerance. For dae, usdz and usda the judges are
//    SceneKit (Collada) and usdrecord (USD), one light at a time on white
//    Lambert surfaces, compared on where it falls and its colour
//    (page/render.js, compareLightPicture): their light units and falloff
//    are their own, and each deviation is cited below.
//  - the numeric oracle, camlight-oracle.mjs: assimp's reading of every fbx,
//    dae, glb and gltf output, and of every expected GLB (its calibration),
//    and every usdz and usda output's cameras read from its flattened text,
//    against the expected scene within 1e-4, adjusted where the reader's
//    convention differs (each citation is in its ASSIMP and USD_TEXT
//    tables); `summary.camlightOracle` counts it. It compares every camera's
//    frame at rest and, on an animated case, at each sample time, so it is
//    the judge of a camera no picture draws through, and of a near or far
//    plane that cuts no geometry, which no picture shows.
//
// Nodes are matched by name; a node with no name by its depth-first rank, under
// each reader's name for it (camlight-oracle.mjs nameCandidates, Ruling R25 as
// amended: page/node-rank.js).
//
// three.js's FBXLoader, ColladaLoader and USDLoader judge neither (the spec's
// "Who decides each mapping"): their cases are skips, citing the loader.
const THREE_LOADERS = 'scripts/mesh-verify/node_modules/three/examples/jsm/loaders';
const CAMERA_NONE = 'the expected scene keeps no camera';
const LIGHTS_NONE = 'the expected scene keeps no light';
const CAMERA_THREE_SKIP = {
  fbx: `three.js FBXLoader is no judge of an FBX camera (spec, "Who decides each mapping"): it turns no camera, so one looks along FBX's +X, 90° off (createCamera, ${THREE_LOADERS}/FBXLoader.js:1064-1153), builds no orthographic camera (:1137-1140), takes the fov from FieldOfView or from FocalLength against its own 35 mm film (:1121-1134) and divides NearPlane and FarPlane by 1000 (:1097, :1104); judged by assimp's reading (camlight-oracle.mjs, Ruling R11)`,
  dae: `three.js ColladaLoader is no judge of a Collada camera (spec, "Who decides each mapping"): it ignores <xfov>, so a camera without <yfov> gets PerspectiveCamera's 50° and one without <aspect_ratio> an aspect of 1 (${THREE_LOADERS}/collada/ColladaComposer.js:1410-1417), and it halves <xmag> and <ymag> (:1427-1428); judged by SceneKit through the file's camera and by assimp's reading`,
  usd: `three.js USDLoader builds no camera: a Camera prim falls into its generic branch and becomes an empty Object3D (${THREE_LOADERS}/usd/USDComposer.js:663-673); judged by SceneKit through the file's camera and by usdrecord --camera`,
};
const LIGHTS_THREE_SKIP = {
  fbx: `three.js FBXLoader is no judge of FBX lights (spec, "Who decides each mapping"): it aims a directional or spot light only at a LookAtProperty target, so one without shines at the origin (setLookAtProperties, ${THREE_LOADERS}/FBXLoader.js:1423-1459), takes a spot's InnerAngle, a full cone, as its half-angle (:1242) and max(rad(OuterAngle), 1) as its penumbra (:1246-1253), and uses decay 1 where the file says the inverse square (:1225); judged by assimp's reading`,
  dae: `three.js ColladaLoader is no judge of Collada lights (spec, "Who decides each mapping"): buildLight sets no target (${THREE_LOADERS}/collada/ColladaComposer.js:1466-1494), so a directional or spot light shines at the origin (src/lights/DirectionalLight.js:58-69), it takes the square root of 1/<quadratic_attenuation> as a cutoff distance (collada/ColladaParser.js:1185-1188, ColladaComposer.js:1491), which cuts the inverse square written here off at 1 m (Ruling R18), makes <falloff_angle>, the full cone (Ruling R23), a SpotLight's angle (ColladaComposer.js:1492), which is a half-angle ("Maximum angle of light dispersion from its direction", src/lights/SpotLight.js:79-85), so it draws every spot twice as wide, ignores <falloff_exponent>, and decodes <color> as sRGB (ColladaParser.js:1175-1178) where it is linear (Ruling R17); judged by SceneKit and by assimp's reading`,
  usd: `three.js USDLoader builds no light: a UsdLux prim falls into its generic branch and becomes an empty Object3D (${THREE_LOADERS}/usd/USDComposer.js:663-673); judged by usdrecord`,
};
// SceneKit's and Storm's readings of a light, each measured or cited.
const SCENEKIT_USD_LIGHTS =
  'SceneKit reads no UsdLux light: Model I/O gives a DistantLight or SphereLight prim as a plain MDLObject, not an MDLLight (measured 2026-11-06 on macOS 26.6.2: MDLAsset holds no light, and SCNScene(mdlAsset:) none, for a stage of SphereLight, DistantLight and ShapingAPI prims written with the inputs: and with the pre-21.02 attribute names alike), so USD lights are judged by usdrecord';
// SceneKit misreads an orthographic camera in both its importers (measured
// 2026-11-06, render.swift's camera read): a Collada one's orthographicScale,
// the half-height of the view, comes out as xmag × ymag (`staged`'s xmag 3,
// ymag 2 read as 6; the Blender FBX reading's 3 and 1.6875 as 5.0625, where
// the half-height is the ymag), and a USD one's as 1 whatever its apertures
// (60 × 40 and 60 × 33.75 tenths of a unit both read as 1).
const SCENEKIT_ORTHO =
  "SceneKit is no judge of an orthographic camera: it reads a Collada one's orthographicScale (the view's half-height) as xmag × ymag (measured: xmag 3, ymag 2 read as 6; 3 and 1.6875 as 5.0625) and a USD one's as 1 whatever its apertures (measured: 60 × 40 and 60 × 33.75 tenths of a unit both read as 1); judged by the oracle (its frame; a USD one's magnifications from its text) and, for USD, by usdrecord --camera";
// SceneKit draws spotOuterAngle as the full cone, though SCNLight.h calls it
// the angle from the spot direction (measured 2026-11-06: an outer angle of
// 30 lights a cone of half-angle 15.14°, 20 one of 9.98°), and its Collada
// importer copies <falloff_angle> into it: it reads <falloff_angle> as the
// full cone, as Blender's Collada IO does, and as litegen writes it (Ruling
// R23, which revised R4 on this measurement). So its spot footprint is
// compared with no adjustment.
const SCENEKIT_LIGHT_CLAMP =
  "SceneKit clamps each channel of a Collada light's <color> to 1 (measured: (2, 0.5, 0.25) reads as (1, 0.5, 0.25)), and this <color> is colour × intensity (Ruling R17) with a channel past 1, so its hue is SceneKit's clamp; the colour is judged by assimp's reading (camlight-oracle.mjs, color × intensity)";
const STORM_SPHERE_4X =
  'Storm draws a normalized SphereLight as a point light of its full intensity (pxr/imaging/hdSt/light.cpp L159-L161, the same at OpenUSD v25.02, v25.05 and v26.11.1), 4× the candela litegen writes it from (Ruling R6, usd_write.rs), so its exposure is 2 stops lower here and the brightness ratio left is recorded';
const STORM_NO_CONE =
  "Storm shapes only rect and disk lights (SetSpotCutoff, pxr/imaging/hdSt/light.cpp L244-L248), so it draws a SphereLight spot with no cone, and the footprint is not usdrecord's to judge (its colour is); SceneKit reads no USD light, so a USD spot's cone has no judge here beyond litegen's own unit tests";
// How each lights judge's expected picture is drawn (page/render.js, the
// light modes), each a reader's measured or cited reading, not a tolerance.
const RANGE_NOT_CARRIED =
  "glTF `range` has no native Collada or USD form (item 5's Ruling R27: it rides only in litegen's annotation), so SceneKit and Storm light past it; the expected picture is drawn without three.js's range window (lights_pars_begin.glsl.js L56-L69)";
const SCENEKIT_SRGB_COLOR =
  "SceneKit decodes a Collada light's <color> as sRGB (measured 2026-11-06 on Khronos DirectionalLight's sun, <color> (0.9, 0.8, 0.1): SceneKit's lit pixels hold (1, 0.7670, 0.0127) over the largest channel, (0.9, 0.8, 0.1) decoded from sRGB is (1, 0.7669, 0.0127), three.js's (1, 0.8890, 0.1110)); litegen's <color> is linear (Rulings R17 and R26), so SceneKit's expected picture lights with that colour decoded as sRGB";
const SCENEKIT_NO_FALLOFF =
  "SceneKit gives a Collada point or spot light no distance falloff: it reads <quadratic_attenuation> 1 as attenuation start and end 0 (render.swift's `lights` read), and its brightness over three.js's inverse square grows with the distance (Khronos PlaysetLightTest's LEDlight: p10 1.222, median 1.747, p90 2.284, where the directional light1 sits at 1.000), so SceneKit's expected picture draws the light with no falloff (three.js decay 0)";

// What the FBX judge takes from the expected scene, where assimp holds no
// value of its own.
const ASSIMP_FBX_LIGHTS =
  "assimp holds srgb(colour) × intensity (FBXConverter.cpp L401-L407; litegen's default FbxColorSpace::Srgb, Ruling R14), decoded here with the expected light's intensity, and reads no FarAttenuationEnd (L445-L469 read DecayType and DecayStart alone), so each light's range is the expected scene's";
const ASSIMP_FBX_ORTHO =
  'assimp reads no FBX orthographic camera (FBXConverter.cpp L472-L508 reads no CameraProjectionType or OrthoZoom), so an orthographic first camera has no assimp judge in the camera mode';
// The cross-renderer lights judge's limits (page/render.js,
// compareLightPicture): the share of lit pixels whose lit state differs, and
// the share of hue-bearing pixels whose colour does.
const LIGHT_FOOTPRINT_LIMIT = 0.02;
const LIGHT_HUE_LIMIT = 0.02;
// Asset limits: a measured property of one third-party asset, named by the
// SHA-256 of its file, that keeps a judge from judging it there; each cites
// its evidence (item 5's Task 6b, `.superpowers/sdd/2026-11-04-mesh-lights-
// cameras/t6b/evidence.log` and `experiments/`, and the follow-up round): local
// files in that item's git-ignored ledger, never committed.
const ASSET_LIMITS = {
  // Khronos glTF-Sample-Assets 8f492390, DirectionalLight.glb.
  '82e50feb151203f431a3b43985e9f4b2a54131b61a7a8677954922630d90f3de': {
    usdrecordFootprint: {
      Sun_Orientation:
        "Khronos DirectionalLight's three spheres are inside out: outward normals 0 of 5,374 and outward-facing CCW triangles 0 of 10,600 on each, single-sided materials, and our USD keeps exactly that. three.js and SceneKit cull back faces and draw each sphere's far interior, which faces the camera and the sun; Storm draws the near side, whose normals face away, dark. Storm emits along -Z as UsdLux says: the same file turned 180° lights, and an analytic Sphere under the same DistantLight lights. The sun itself is exact (numerically: direction 0°, colour 8.8e-8)",
    },
  },
  // Khronos glTF-Sample-Assets 8f492390, LightsPunctualLamp.glb.
  b9a95cd0e17dea5284117e54807b49b746960038457a31a0011b9344acc11104: {
    usdrecordFootprint: {
      'Point.003_Orientation':
        "Storm samples each pixel 4 times (coverage alpha in quarters, 0/64/128/191/255, measured on this file) and three.js once (antialias off); Point.003 sits under the lamp's finely detailed stem and grazes rims thinner than a pixel, which Storm's samples catch and three.js's pixel centre misses: of the 88 differing pixels (8.59 %), 68 are partially covered edges and 20 faint slivers (4-91 of 224) inside the silhouette. Measured, none closes it: the light's specular off 8.59 %; the expected drawn double-sided 7.80 %; the expected supersampled 2x2 6.69 %, 4x4 4.80 %. The light is exact (numerically: position 3.0e-8), and SceneKit's footprint of it in Collada is 0.24 %",
    },
  },
  // litegen-core/src/mesh/testdata/gltf/gltfpack/layered.cz.glb (gltfpack 1.3.0 -cz: KHR_meshopt_compression, v1 streams). Mesh-gaps item 2, Ruling B46.
  '909596012abc9117e6ed7d9b6e19e5195c8d0f136ee15f1852cb985eb17cb535': {
    nativeCheck: {
      three:
        "three r183's bundled meshopt decoder (examples/jsm/libs/meshopt_decoder.module.js, 'Built from meshoptimizer 1.0', wasm) mis-decodes this file's bufferView 3 (ATTRIBUTES, filter NONE, v1 header 0xa1, 72 elements x 8 bytes: the quantized u16 positions): 60 of its 576 bytes differ from meshoptimizer's reference decoder (js/meshopt_decoder_reference.js at f0a684f), while the other 136 meshopt views of the 27 compressed gltfpack files decode byte-identically in both (137 views, 136 identical, 2026-11-08); dequantized, the reference decode lands on the 168 vertices of the repo's uncompressed litegen-core/src/mesh/testdata/gltf/gltfpack/layered.glb and three's is off on 156 of 168 (worst 12.4 units), so its picture of this file (a sliver and four squares, 93.5 % of geom pixels apart) cannot judge our reading, which matches the reference decoder (Task 2's differential). Measured 2026-11-08 with scripts/mesh-verify README's decoder comparison; every other check on this file runs",
    },
  },
};
const assetHashes = new Map();
async function assetLimit(fx, judge, light) {
  if (!fx.native?.path) return null;
  const file = path.join(matrixDir, fx.native.path);
  if (!assetHashes.has(file)) assetHashes.set(file, crypto.createHash('sha256').update(await fs.readFile(file)).digest('hex'));
  return ASSET_LIMITS[assetHashes.get(file)]?.[judge]?.[light] ?? null;
}
const camlightNoJudge = (what) => `no judge of ${what.join(', ')}: every judge of it was skipped, in every frame, or did not run`;
const camlightTotals = { files: 0, failed: 0, passed: 0, failedChecks: 0, skipped: 0, byFormat: {}, expectedFiles: 0, expectedFailed: 0 };

// Which camera and light modes three.js draws for a case (`three`), and the
// skips, with why.
function camlightPlan(c, info) {
  const plan = { camera: !!info?.cameras?.length, lights: !!info?.lights?.length, three: [], skips: {} };
  if (!plan.camera) plan.skips.camera = CAMERA_NONE;
  if (!plan.lights) plan.skips.lit_lights = LIGHTS_NONE;
  const k = USD_FORMATS[c.to] ? 'usd' : c.to;
  if (GLTF_NATIVE[c.to]) {
    if (plan.camera) plan.three.push('camera');
    if (plan.lights) plan.three.push('lit_lights');
  } else {
    if (plan.camera) plan.skips.camera = CAMERA_THREE_SKIP[k] ?? `three.js reads no ${c.to} camera`;
    if (plan.lights) plan.skips.lit_lights = LIGHTS_THREE_SKIP[k] ?? `three.js reads no ${c.to} light`;
  }
  return plan;
}

// The numeric oracle on one case: assimp's reading of an FBX, Collada, GLB or
// glTF output (with assimp's document, `aj`, for the FBX judge), or a USD
// output's cameras read from its flattened text (usdCameraFile). The camera
// pictures show a near or far plane only where it cuts geometry, so these
// numbers are the judge of the clip planes.
const ORACLE_FORMATS = { fbx: 'assimp', dae: 'assimp', glb: 'assimp', gltf: 'assimp', usdz: 'usd', usda: 'usd', usdc: 'usd' };
async function camlightOracleCase(entry, c, primary, times) {
  const how = ORACLE_FORMATS[c.to];
  if (!how || (how === 'assimp' && !TOOLS.assimp) || (how === 'usd' && !TOOLS.usdcat)) return null;
  const expectedGlb = path.join(matrixDir, manifest.fixtures[c.fixture].expected[c.expected]);
  const r =
    how === 'usd'
      ? await usdCameraFile(primary, expectedGlb, times)
      : await oracleFile(primary, expectedGlb, path.join(outDir, 'assjson', c.fixture, `${caseId(c)}.assjson`), times);
  if (!r.error && !r.want.cameras.length && !r.want.lights.length && !r.got.cameras.length && !r.got.lights.length) return r;
  camlightTotals.files++;
  camlightTotals.byFormat[c.to] = (camlightTotals.byFormat[c.to] ?? 0) + 1;
  camlightTotals.passed += r.passed;
  camlightTotals.failedChecks += r.failed;
  camlightTotals.skipped += r.skipped;
  entry.camlightOracle = { format: r.format, passed: r.passed, failed: r.failed, skipped: r.skipped, ...(r.error ? { error: r.error } : {}), ...(r.note ? { note: r.note } : {}), checks: r.checks };
  if (r.error) entry.reasons.push(`camlight oracle: ${r.error}`);
  for (const x of r.checks.filter((k) => k.verdict === 'fail')) {
    entry.reasons.push(`camlight oracle: ${x.subject} / ${x.field}: ${x.degrees !== undefined ? `${x.degrees.toFixed(3)}° apart` : x.reason ?? `${JSON.stringify(x.want)} -> ${JSON.stringify(x.got)}`}`);
  }
  if (r.error || r.failed) camlightTotals.failed++;
  return r;
}

// Store a judge's per-frame results under `key`, saving its pictures and
// failing the case past `limit` (as the three.js results are).
async function judgeFrames(entry, key, byT, dir, limit) {
  entry.results[key] = {};
  for (const [t, res] of Object.entries(byT || {})) {
    const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, maxDelta: res.maxDelta };
    const file = key.replace(/:/g, '_');
    if (res.skipped) {
      item.skipped = res.skipped;
    } else if (res.error) {
      item.error = res.error;
      entry.reasons.push(`${key}@${t}: ${res.error}`);
    } else {
      item.image = await saveDataUrl(res.image, path.join(dir, `${file}_${t}.png`));
      if (res.ratio > limit) {
        item.diff = await saveDataUrl(res.diff, path.join(dir, `${file}_${t}_diff.png`));
        entry.reasons.push(`${key}@${t}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
      }
    }
    entry.results[key][t] = item;
  }
}

// The FBX judge (Ruling R11): the expected GLB drawn through a camera and lit
// by lights built from assimp's reading of the file (camlight-oracle.mjs,
// assimpView), at each time, against the expected pictures.
async function assimpCamlight(entry, c, fx, oracle, plan, info) {
  const dir = path.join(outDir, 'img', c.fixture, caseId(c));
  if (!oracle?.aj) {
    for (const [on, key] of [[plan.camera, 'assimp:camera'], [plan.lights, 'assimp:lit_lights']]) {
      if (on) entry.results[key] = { static: { error: oracle?.error ?? 'assimp is not installed' } };
    }
    return;
  }
  const times = c.caps.animation ? fx.times : [];
  const view = assimpView(oracle.aj, 'fbx', times);
  const external = {};
  const modes = [];
  if (plan.camera) {
    // The expected scene's first camera, by its node's name (assimp names a
    // camera after its node, FBXConverter.cpp L476), or a nameless node's.
    const name = info.cameras[0].name;
    const names = nameCandidates(name);
    const first = oracle.want.cameras.find((x) => x.name === name);
    const cam = view.cameras.find((x) => names.includes(x.name));
    if (!cam) entry.results['assimp:camera'] = { static: { error: `assimp read no camera on ${name}` } };
    else if (first?.projection === 'orthographic') entry.results['assimp:camera'] = { static: { skipped: ASSIMP_FBX_ORTHO } };
    else {
      external.camera = cam;
      modes.push('camera');
    }
  }
  if (plan.lights) {
    external.lights = view.lights
      .filter((l) => ['directional', 'point', 'spot'].includes(l.kind))
      .map((l) => {
        const w = oracle.want.lights.find((x) => nameCandidates(x.name).includes(l.name));
        const intensity = w?.intensity ?? Math.max(...l.product);
        const color = l.product.map((p) => srgbDecode(intensity > 0 ? p / intensity : 0));
        return { kind: l.kind, color, intensity, distance: w?.range ?? 0, inner: l.inner, outer: l.outer, frames: l.frames };
      });
    entry.assimpLights = ASSIMP_FBX_LIGHTS;
    modes.push('lit_lights');
  }
  if (!modes.length) return;
  const r = await page.evaluate((j) => window.renderCase(j), {
    url: `${base}/m/${manifest.fixtures[c.fixture].expected[c.expected]}`,
    format: 'glb',
    files: [],
    camera: fx.camera,
    modes,
    times,
    expectedKey: `${c.fixture}|${c.expected}`,
    clipName: fx.animations?.[0],
    external,
  });
  if (r.error) {
    entry.reasons.push(`assimp judge: ${r.error}`);
    return;
  }
  for (const mode of modes) await judgeFrames(entry, `assimp:${mode}`, r.results[mode], dir, THRESHOLD[mode]);
}

// Every judge draws through the camera on the node of the expected scene's
// first camera (`info.cameras[0]`, its node's name, or for a node with no
// name each reader's name for its rank, nameCandidates), never by its place
// in a reader's list: a reader may order cameras its own way (SceneKit's USD
// importer lists a prim's children alphabetically, render.swift's
// cameraNodes), and GLTFLoader clones a camera two nodes share onto each.
// The flattened stage's prims come from camlight-oracle.mjs (usdPrims).
// A node's name in a picture's file name, which has no space.
const fileTag = (name) => name.replace(/[^\w.-]+/g, '_');
// Session-layer overs on a stage's prims, as one tree (a prim twice in one
// layer is an error): `attrs` inside each prim's block, `meta` in its
// metadata, both at the stage's own paths.
function usdOvers(entries) {
  const tree = {};
  for (const e of entries) {
    let node = tree;
    for (const part of e.path.split('/').filter(Boolean)) node = node[part] ??= {};
    // A prim reached twice (two materials sharing a shader) is written once.
    if (e.attrs) node['\0'] = [...new Set([...(node['\0'] || []), ...e.attrs])];
    if (e.meta) node['\u0001'] = [...new Set([...(node['\u0001'] || []), ...e.meta])];
  }
  const out = [];
  const emit = (node, depth) => {
    const pad = '    '.repeat(depth);
    for (const [k, v] of Object.entries(node)) {
      if (k === '\0' || k === '\u0001') continue;
      out.push(v['\u0001'] ? `${pad}over "${k}" (\n${v['\u0001'].map((x) => `${pad}    ${x}`).join('\n')}\n${pad})` : `${pad}over "${k}"`, `${pad}{`);
      for (const l of v['\0'] || []) out.push(`${pad}    ${l}`);
      emit(v, depth + 1);
      out.push(`${pad}}`);
    }
  };
  emit(tree, 0);
  return out.join('\n') + '\n';
}
// Every light prim of a flattened stage, by path, and the overs that switch
// them off: a pass that judges something other than the file's lights draws
// none of them (usdrecord otherwise lights its albedo with the stage's
// lights: Khronos PlaysetLightTest's 512.25 lux sun washed it white, 93.30 %
// of the pixels past 16/255 with the lights on, 9.47 % off; task 6b's
// experiments/usdrecord-albedo).
const usdLightPaths = (flat) => usdPrims(flat).filter((p) => /Light$/.test(p.type)).map((p) => p.path);
const usdLightsOff = (paths) => paths.map((p) => ({ path: p, meta: ['active = false'] }));

// Session-layer overs for usdrecord's lights judge, at the stage's own paths:
// every surface shader white Lambert (no specular: ior 1, no emission, no
// maps), every light prim but `keep` deactivated, and `keep` at `exposure`.
function usdLightOvers(text, lights, keep, exposure) {
  const entries = [];
  for (const m of text.matchAll(/outputs:surface\.connect = <([^>]+)\.outputs:surface>/g)) {
    entries.push({ path: m[1], attrs: [
      'color3f inputs:diffuseColor.connect = None',
      'color3f inputs:diffuseColor = (1, 1, 1)',
      'color3f inputs:emissiveColor.connect = None',
      'color3f inputs:emissiveColor = (0, 0, 0)',
      'float inputs:ior = 1',
      'float inputs:roughness = 1',
      'float inputs:metallic.connect = None',
      'float inputs:metallic = 0',
      'float inputs:clearcoat = 0',
      'float inputs:opacity.connect = None',
      'float inputs:opacity = 1',
      'int inputs:useSpecularWorkflow = 0',
      'normal3f inputs:normal.connect = None',
      'float inputs:occlusion.connect = None',
    ] });
  }
  for (const l of lights) {
    if (l === keep) entries.push({ path: l.path, attrs: [`float inputs:exposure = ${exposure}`] });
    else entries.push(...usdLightsOff([l.path]));
  }
  return usdOvers(entries);
}

// The cross-renderer lights judge's verdict on one picture: the footprint
// unless `footprintSkip`, the hue unless `hueSkip`.
async function judgeLight(entry, label, key, cmp, dir, file, footprintSkip, hueSkip) {
  if (cmp.error) {
    entry.reasons.push(`${label}@${key}: ${cmp.error}`);
    return { error: cmp.error };
  }
  const item = { footprint: cmp.footprint, hue: cmp.hue, brightness: cmp.brightness, image: await saveDataUrl(cmp.image, path.join(dir, `${file}_${key}.png`)) };
  if (cmp.diff) item.diff = await saveDataUrl(cmp.diff, path.join(dir, `${file}_${key}_diff.png`));
  // A light that lights nothing the fixture's camera sees, in the expected
  // picture and in the judge's alike, leaves nothing to judge in this frame;
  // lit on one side only, it is judged, and every lit pixel differs.
  const dark = !cmp.footprint.wantLit && !cmp.footprint.gotLit && 'the light lights nothing the fixture camera sees, in the expected picture or in this one';
  if (footprintSkip || dark) item.footprint.skipped = footprintSkip || dark;
  else if (cmp.footprint.ratio > LIGHT_FOOTPRINT_LIMIT) entry.reasons.push(`${label}@${key}: the light falls differently on ${(cmp.footprint.ratio * 100).toFixed(2)}% of the lit pixels`);
  if (hueSkip) item.hue.skipped = hueSkip;
  else if (cmp.hue.ratio > LIGHT_HUE_LIMIT) entry.reasons.push(`${label}@${key}: the light's colour differs on ${(cmp.hue.ratio * 100).toFixed(2)}% of the pixels that carry it`);
  // A judged frame: something was compared and not skipped.
  item.judged = !item.footprint.skipped || (!hueSkip && cmp.hue.pixels > 0);
  return item;
}

// SceneKit's camera and lights judges, for Collada and USD.
async function scenekitCamlight(entry, c, fx, primary, plan, info) {
  if (!SCENEKIT_FORMATS.has(c.to) || !(c.to === 'dae' || USD_FORMATS[c.to]) || !(await scenekitBinary())) return;
  entry.scenekit = entry.scenekit || {};
  const dir = path.join(outDir, 'img', c.fixture, caseId(c));
  await fs.mkdir(dir, { recursive: true });
  const times = [null, ...(c.caps.animation ? fx.times : [])];
  const expectedKey = `${c.fixture}|${c.expected}`;
  let clip = null;
  let notPlayed = null;
  if (c.to === 'dae' && times.length > 1) ({ clip, notPlayed } = sceneKitClip(await fs.readFile(primary, 'utf8'), fx.animations?.[0]));
  const url = (png, retry) => `${base}/out/${path.relative(outDir, png)}${retry ? '?retry=1' : ''}`;
  if (plan.camera && info.cameras[0].kind === 'orthographic') {
    entry.scenekit.camera = { skipped: SCENEKIT_ORTHO };
  } else if (plan.camera) {
    // The expected scene's first camera, by its node's names (above).
    const cameraNodes = nameCandidates(info.cameras[0].name);
    const { width, height } = info.cameraView;
    entry.scenekit.camera = {};
    for (const t of times) {
      const key = t === null ? 'static' : String(t);
      if (t !== null && notPlayed) {
        entry.scenekit.camera[key] = { skipped: notPlayed };
        continue;
      }
      const png = path.join(dir, `scenekit_camera_${key}.png`);
      const job = { view: 'file', cameraNodes, width, height };
      let r = await scenekitRender(primary, fx.camera, 'silhouette', t, png, clip, null, job);
      if (!r || r.error) {
        entry.scenekit.camera[key] = { error: r ? r.error : 'renderer unavailable' };
        entry.reasons.push(`SceneKit camera@${key}: ${r ? r.error : 'renderer unavailable'}`);
        continue;
      }
      const compare = (retry) => page.evaluate(([u, k, tk, o]) => window.compareImage(u, k, 'silhouette', tk, o), [url(png, retry), expectedKey, key, { wantMode: 'camera', width, height }]);
      let cmp = await compare(false);
      let retried = false;
      // An empty frame where a picture is expected is rendered once more,
      // as for SceneKit's other modes.
      if (!cmp.error && cmp.gotDrawn === 0 && cmp.wantDrawn > 0) {
        const again = await scenekitRender(primary, fx.camera, 'silhouette', t, png, clip, null, job);
        if (again && !again.error) {
          r = again;
          cmp = await compare(true);
          retried = true;
        }
      }
      const item = { image: path.relative(outDir, png), ratio: cmp.ratio, camera: r.stats?.camera, ...(retried ? { retried: 'SceneKit drew an empty frame; rendered again' } : {}) };
      if (cmp.error) {
        item.error = cmp.error;
        entry.reasons.push(`SceneKit camera@${key}: ${cmp.error}`);
      } else if (cmp.ratio > 0.01) {
        item.diff = await saveDataUrl(cmp.diff, path.join(dir, `scenekit_camera_${key}_diff.png`));
        entry.reasons.push(`SceneKit camera@${key}: ${(cmp.ratio * 100).toFixed(2)}% differ`);
      }
      entry.scenekit.camera[key] = item;
    }
  }
  if (!plan.lights) return;
  if (USD_FORMATS[c.to]) {
    entry.scenekit.lights = { skipped: SCENEKIT_USD_LIGHTS };
    return;
  }
  const json = readGltfJson(await fs.readFile(path.join(matrixDir, manifest.fixtures[c.fixture].expected[c.expected])));
  const products = new Map(sceneLights(json).map((l) => [l.name, l.color.map((x) => x * l.intensity)]));
  const ranges = new Map(sceneLights(json).map((l) => [l.name, l.range]));
  for (const l of info.lights) {
    const key0 = `light:${l.name}`;
    entry.scenekit[key0] = {};
    const clamped = (products.get(l.name) || []).some((x) => x > 1);
    for (const t of times) {
      const key = t === null ? 'static' : String(t);
      if (t !== null && notPlayed) {
        entry.scenekit[key0][key] = { skipped: notPlayed };
        continue;
      }
      const png = path.join(dir, `scenekit_light_${fileTag(l.name)}_${key}.png`);
      const r = await scenekitRender(primary, fx.camera, 'lights', t, png, clip, null, { lightNodes: nameCandidates(l.name) });
      if (!r || r.error) {
        entry.scenekit[key0][key] = { error: r ? r.error : 'renderer unavailable' };
        entry.reasons.push(`SceneKit ${key0}@${key}: ${r ? r.error : 'renderer unavailable'}`);
        continue;
      }
      // Against the expected picture drawn as SceneKit draws a Collada light
      // (page/render.js `lightsk:`): its colour read as sRGB, no falloff for
      // a point or spot, no range.
      const cmp = await page.evaluate((j) => window.compareLightPicture(j), { url: url(png), expectedKey, mode: `lightsk:${l.name}`, t: key });
      const item = await judgeLight(entry, `SceneKit ${key0}`, key, cmp, dir, `scenekit_light_${fileTag(l.name)}_judged`, null, clamped ? SCENEKIT_LIGHT_CLAMP : null);
      item.drawnAs = [SCENEKIT_SRGB_COLOR, ...(l.kind === 'point' || l.kind === 'spot' ? [SCENEKIT_NO_FALLOFF] : []), ...(ranges.get(l.name) !== undefined ? [RANGE_NOT_CARRIED] : [])];
      item.exposure = r.stats?.exposure;
      item.read = (r.stats?.lights || []).find((x) => nameCandidates(l.name).includes(x.node));
      entry.scenekit[key0][key] = item;
    }
  }
}

// usdrecord's camera and lights judges, for USD.
async function usdrecordCamlight(entry, c, fx, primary, plan, info) {
  if (!USD_FORMATS[c.to] || !TOOLS.usdrecord || !TOOLS.usdcat) return;
  let text;
  try {
    text = (await run('usdcat', ['--flatten', primary], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  } catch (e) {
    entry.reasons.push(`usdcat cannot flatten the file: ${String(e.stderr || e.message).slice(0, 200)}`);
    return;
  }
  const prims = usdPrims(text);
  const tcps = Number((text.match(/timeCodesPerSecond = ([0-9.eE+-]+)/) || [])[1] || 24);
  const times = c.caps.animation ? fx.times : [];
  const work = path.join(matrixDir, c.dir, 'usdrecord_camlight');
  const rel = path.relative(matrixDir, work).split(path.sep).join('/');
  const dir = path.join(outDir, 'img', c.fixture, caseId(c));
  const expectedKey = `${c.fixture}|${c.expected}`;
  if (plan.camera) {
    // The expected scene's first camera, by its node's names.
    const name = info.cameras[0].name;
    const names = nameCandidates(name);
    const first = prims.find((p) => p.type === 'Camera' && names.includes(p.name));
    entry.results['usdrecord:camera'] = {};
    if (!first) {
      entry.results['usdrecord:camera'].static = { error: `the stage has no Camera prim named ${name}` };
      entry.reasons.push(`usdrecord:camera: the stage has no Camera prim named ${name}`);
    } else {
      // usdrecord draws a camera at the camera's own aspect, and litegen
      // writes a camera with no aspect ratio to USD as 16:9 (usd_write.rs
      // `write_camera`; 3d-format-conversion.md §5, reported as a loss),
      // so such a camera is compared with the expected picture drawn at that
      // aspect (page/render.js, `<key>#aspect`). Any other size mismatch
      // stays an error (compareExternal).
      const wide = info.cameraAspectUnset ? await page.evaluate((k) => window.expectedStats(k)?.cameraView ?? null, `${expectedKey}#aspect`) : null;
      const { width, height } = wide ?? info.cameraView;
      const coverKey = wide ? `${expectedKey}#aspect` : expectedKey;
      const frames = await usdrecordFrames(primary, `#usda 1.0\n${usdOvers(usdLightsOff(usdLightPaths(text)))}`, times, tcps, work, 'camera', [], { camera: first.path, width });
      for (const [label, file] of Object.entries(frames)) {
        if (!file) {
          entry.results['usdrecord:camera'][label] = { error: 'usdrecord failed' };
          entry.reasons.push(`usdrecord:camera@${label}: usdrecord failed`);
          continue;
        }
        const res = await page.evaluate((j) => window.compareExternal(j), { url: `${base}/m/${rel}/${path.basename(file)}`, expectedKey: coverKey, mode: 'camera_cover', t: label, kind: 'silhouette', width, height });
        const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, camera: first.path, ...(wide ? { aspect: 'the expected camera has none; litegen writes 16:9 (UNSET_ASPECT)' } : {}), ...(res.rescaled ? { rescaled: res.rescaled } : {}) };
        if (res.error) {
          item.error = res.error;
          entry.reasons.push(`usdrecord:camera@${label}: ${res.error}`);
        } else {
          item.image = await saveDataUrl(res.image, path.join(dir, `usdrecord_camera_${label}.png`));
          if (res.ratio > USD_THRESHOLD.cover) {
            item.diff = await saveDataUrl(res.diff, path.join(dir, `usdrecord_camera_${label}_diff.png`));
            entry.reasons.push(`usdrecord:camera@${label}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
          }
        }
        entry.results['usdrecord:camera'][label] = item;
      }
    }
  }
  if (!plan.lights) return;
  const lights = prims.filter((p) => /Light$/.test(p.type));
  const ranges = new Map(sceneLights(readGltfJson(await fs.readFile(path.join(matrixDir, manifest.fixtures[c.fixture].expected[c.expected])))).map((x) => [x.name, x.range]));
  for (const l of info.lights) {
    const key0 = `usdrecord:light:${l.name}`;
    entry.results[key0] = {};
    const prim = lights.find((p) => nameCandidates(l.name).includes(p.name));
    if (!prim) {
      entry.results[key0].static = { error: `the stage has no light prim named ${l.name}` };
      entry.reasons.push(`${key0}: the stage has no light prim named ${l.name}`);
      continue;
    }
    // The expected picture's exposure, less Storm's cited 4x for a sphere
    // light, on top of any the prim authors.
    const authored = Number((prim.body.match(/inputs:exposure = ([0-9.eE+-]+)/) || [])[1] || 0);
    const exposure = Math.log2(info.lightExposure?.[l.name] ?? 1) + authored - (prim.type === 'SphereLight' ? 2 : 0);
    const session = usdCameraLayer(fx.camera) + usdLightOvers(text, lights, prim, exposure);
    const frames = await usdrecordFrames(primary, session, times, tcps, work, `light_${fileTag(l.name)}`, [USDRECORD_NO_CAMERA_LIGHT]);
    for (const [label, file] of Object.entries(frames)) {
      if (!file) {
        entry.results[key0][label] = { error: 'usdrecord failed' };
        entry.reasons.push(`${key0}@${label}: usdrecord failed`);
        continue;
      }
      const cmp = await page.evaluate((j) => window.compareLightPicture(j), { url: `${base}/m/${rel}/${path.basename(file)}`, expectedKey, mode: `light:${l.name}`, t: label });
      const limit = await assetLimit(fx, 'usdrecordFootprint', l.name);
      const item = await judgeLight(entry, key0, label, cmp, dir, `usdrecord_light_${fileTag(l.name)}`, l.kind === 'spot' ? STORM_NO_CONE : limit, null);
      if (prim.type === 'SphereLight') item.storm = STORM_SPHERE_4X;
      if (ranges.get(l.name) !== undefined) item.drawnAs = [RANGE_NOT_CARRIED];
      item.exposure = exposure;
      entry.results[key0][label] = item;
    }
  }
}

// Each camera and each light of the expected scene ends with a judge of its
// own (camlightNoJudge names any that does not). A camera's frame: the
// numeric oracle's eye, forward and up checks of it, at rest or at a sample
// time (its yfov, aspect and planes are its own values, not its frame), or,
// for the camera the pictures draw through, a camera picture judged in some
// frame. A light: the numeric oracle's checks of it passing (its position and
// direction among them), its own one-light picture (SceneKit, usdrecord)
// judged in some frame, or the whole-scene `lit_lights` picture (three.js,
// or assimp's reading) judged in a frame where that light lights something
// the fixture's camera sees (the expected `light:<node>` pictures count its
// lit pixels, page/render.js `lightLit`). A light that is dark in every
// frame is not judged by a picture of the others.
function camlightJudged(entry, plan, info) {
  const oracle = (subject) => (entry.camlightOracle?.checks || []).some((x) => x.verdict === 'pass' && x.subject === subject);
  const framed = (subject) => (entry.camlightOracle?.checks || []).some((x) => x.subject === subject && x.verdict !== 'skip' && /^(?:eye|forward|up)(?:@|$)/.test(x.field));
  const judgedTimes = (byFrame) =>
    byFrame && !byFrame.skipped ? Object.entries(byFrame).filter(([, x]) => x && typeof x === 'object' && !x.skipped && !x.error).map(([t]) => t) : [];
  const ownPicture = (byFrame) => !!byFrame && Object.values(byFrame).some((x) => x && typeof x === 'object' && x.judged);
  const unjudged = [];
  if (plan.camera) {
    // The camera pictures draw through the expected scene's first camera.
    const pictured = ['camera', 'assimp:camera', 'usdrecord:camera'].some((m) => judgedIn(entry.results[m])) || judgedIn(entry.scenekit?.camera);
    for (const cam of info.cameras) {
      if (!framed(`camera ${cam.name}`) && !(cam === info.cameras[0] && pictured)) unjudged.push(`camera ${cam.name}`);
    }
  }
  if (plan.lights) {
    const whole = ['lit_lights', 'assimp:lit_lights'].flatMap((m) => judgedTimes(entry.results[m]));
    for (const l of info.lights) {
      const own = ownPicture(entry.results[`usdrecord:light:${l.name}`]) || ownPicture(entry.scenekit?.[`light:${l.name}`]);
      const lit = info.lightLit?.[l.name] || {};
      const inWhole = whole.some((t) => (lit[t] ?? lit.static ?? 0) > 0);
      if (!oracle(`light ${l.name}`) && !own && !inWhole) unjudged.push(`light ${l.name}`);
    }
  }
  if (unjudged.length) entry.reasons.push(camlightNoJudge(unjudged));
}

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
// xmllint serves the dae-keep variant alone: probed when a dae-keep case first
// needs it, and reported (summary.tools) only for a run that has such cases, so
// a plain run's output is as it was.
let xmllintProbe = null;
const haveXmllint = () => (xmllintProbe ??= haveTool('xmllint'));

// How this usdrecord asks for the default time code. OpenUSD's Python
// usdrecord takes a bare flag (`store_true` in framesArgs.py); the CLI11 build
// in /usr/bin (Apple USD Tools 0.25.2, as found on 2026-10-31) lists
// `-d,--defaultTime BOOLEAN` and reads a bare flag's next argument as its
// value, so every static render failed ("Could not convert: --defaultTime =").
async function usdrecordDefaultTimeArg() {
  if (!TOOLS.usdrecord) return '--defaultTime';
  try {
    const { stdout, stderr } = await run('usdrecord', ['--help']);
    return /--defaultTime\s+BOOLEAN/.test(`${stdout}${stderr}`) ? '--defaultTime=1' : '--defaultTime';
  } catch (e) {
    return /--defaultTime\s+BOOLEAN/.test(`${e.stdout ?? ''}${e.stderr ?? ''}`) ? '--defaultTime=1' : '--defaultTime';
  }
}
const USDRECORD_DEFAULT_TIME = await usdrecordDefaultTimeArg();
// How this usdrecord turns its camera light off (CAMLIGHT's lights judge,
// which must see the file's lights alone). OpenUSD's Python usdrecord takes a
// bare flag, which clears FrameRecorder's camera light
// (pxr/usdImaging/usdAppUtils/frameRecorder.cpp L410-L417 at v25.05). The
// CLI11 build in /usr/bin (Apple USD Tools 0.25.2) reads a value instead, and
// only `=0` turns the light off: measured 2026-11-06 on `staged` with every
// light prim deactivated, the bare flag, `=1` and `=true` render it lit
// (median 178 of 255, as with no flag at all), `=0` renders it black.
const USDRECORD_NO_CAMERA_LIGHT = USDRECORD_DEFAULT_TIME === '--defaultTime=1' ? '--disableCameraLight=0' : '--disableCameraLight';

// ─── SceneKit (second renderer, macOS only) ─────────────────────────────────
//
// Apple's SceneKit reads USD/USDZ, Collada, OBJ+MTL, PLY and STL through
// Model I/O. Built on demand from scenekit/render.swift; skipped silently
// elsewhere. Its pictures are compared with the SAME expected picture the
// three.js renders are, in two modes: silhouette (coverage — geometry,
// transforms, skinning, morphs, animation) and albedo (colour x texture x
// vertex colour), with a looser albedo tolerance because two renderers filter
// textures differently even when the data is identical.
const SCENEKIT_FORMATS = new Set(['obj', 'ply', 'stl', 'usdz', 'usda', 'usdc', 'dae']);
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
const SCENEKIT_SKIP = { 'usdz:albedo': SCENEKIT_USD_ALBEDO, 'usda:albedo': SCENEKIT_USD_ALBEDO, 'usdc:albedo': SCENEKIT_USD_ALBEDO };
// And SceneKit's USD importer sets blend-shape weights only through
// animation: a SkelAnimation's default `blendShapeWeights` is never applied
// (measured on the morph fixture: authored [0.25, 0], SCNMorpher weights
// [0, 0]), so with the clip removed for the rest pose every weight is 0.
// usdrecord, which evaluates the default time, judges that pose.
const SCENEKIT_USD_MORPH_DEFAULTS =
  "SceneKit's USD importer ignores a SkelAnimation's default blendShapeWeights; the default pose is judged by usdrecord";
// So no reader judges the uvs of a USD file with BlendShapes apart from its
// pose. three.js cannot morph it at all, and SceneKit's rest pose is skipped
// (above), while its animated frames are pictures of the pose: a BlendShape
// offsets points and normals, never uvs, so a SceneKit uv picture there
// measures blend-shape evaluation against a limit meant for sub-texel
// sampling (measured: the morph fixture's worst frame read 1.68 % in uv where
// its silhouette read 0.04 %). Its uv sets are skipped with this reason; the
// pose itself is judged by usdrecord's and SceneKit's silhouettes.
const USD_BLENDSHAPE_UV =
  "no reader judges the uvs of a USD file with BlendShapes apart from its pose: three.js USDLoader has no BlendShape support (its skel binding reads skel:skeleton, skel:joints and geomBindTransform and never skel:blendShapes, scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:1148-1176; its SkelAnimation reads rotations, translations and scales and never blendShapeWeights, :3857, :3900, :3940), and SceneKit ignores a SkelAnimation's default blendShapeWeights (SCENEKIT_USD_MORPH_DEFAULTS) while its animated frames judge the pose, which a BlendShape moves and the uvs do not; the pose is judged by usdrecord's and SceneKit's silhouettes";
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

// `channel`: the uv set the `uv` mode draws through (SceneKit's mappingChannel).
// `extra`: render.swift's other job fields (CAMLIGHT: view, cameraNodes,
// width, height, lightNodes).
async function scenekitRender(file, camera, mode, time, out, clip = null, channel = null, extra = {}) {
  const bin = await scenekitBinary();
  if (!bin) return null;
  const jobFile = out + '.job.json';
  await fs.writeFile(
    jobFile,
    JSON.stringify({ camera, size: 256, mode, time, clip, channel, vertexColorsAreSRGB: /\.(obj|ply)$/i.test(file), ...extra }),
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

// SceneKit's albedo verdict (Ruling A38): the share of 4x4 blocks past the
// albedo tolerance (page/render.js, compareImageBlocks), judged against
// SCENEKIT_ALBEDO_BLOCK_LIMIT, with the per-pixel ratio kept beside it. Where
// no block is covered throughout on both sides, the per-pixel verdict stands.
async function scenekitAlbedoVerdict(pngUrl, expectedKey, key, perPixel) {
  const b = await page.evaluate(([u, k, t]) => window.compareImageBlocks(u, k, t), [pngUrl, expectedKey, key]);
  if (b.error || !b.blocks) {
    return { ratio: perPixel.ratio, limit: 0.02, pixelRatio: perPixel.ratio, blocks: b.error ? { error: b.error } : { blocks: 0 }, judge: 'pixels', diff: perPixel.diff };
  }
  return { ratio: b.ratio, limit: SCENEKIT_ALBEDO_BLOCK_LIMIT, pixelRatio: perPixel.ratio, blocks: { blocks: b.blocks, differ: b.differ, maxDelta: b.maxDelta }, judge: 'blocks', diff: b.diff };
}

// The root layer's text: the file itself, or a USDZ's first (stored) entry. A
// crate (a .usdc, or a USDZ whose first entry is one: `usdz-crate`) has no text
// of its own, so it is what usdcat prints for it (the layer alone, not
// flattened: `usdcat x.usdz` prints a package's root layer).
async function usdRootText(file) {
  const b = await fs.readFile(file);
  const crate = file.endsWith('.usdc') || (file.endsWith('.usdz') && b.subarray(30 + b.readUInt16LE(26) + b.readUInt16LE(28), 38 + b.readUInt16LE(26) + b.readUInt16LE(28)).toString('latin1') === 'PXR-USDC');
  if (crate) return (await run('usdcat', [file], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  if (!file.endsWith('.usdz')) return b.toString('utf8');
  const size = b.readUInt32LE(18);
  const start = 30 + b.readUInt16LE(26) + b.readUInt16LE(28);
  return b.subarray(start, start + size).toString('utf8');
}

// `extra`: more prims for the root (a dome light), written before it closes.
function usdCameraLayer(cam, up = [0, 1, 0], extra = '') {
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
${extra}}
`;
}

// Overs that make every UsdPreviewSurface emit exactly its diffuse source and
// reflect nothing: no diffuse, IOR 1 (zero Fresnel reflectance), no normal map;
// and the light prims at `off` (the stage's own, usdLightPaths) switched off.
function usdAlbedoOverrides(text, off = []) {
  const entries = [];
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
    // An unlit material (mesh-gaps item 6: `litegen unlit = 1` in its
    // customData; diffuse black, metallic 1, the base colour in emissiveColor)
    // already emits its base colour and nothing else, so its emission stays as
    // authored: routing its black diffuse to emission would blank it.
    const material = shader.split('/').slice(-2)[0];
    const matAt = text.lastIndexOf(`def Material "${material}"`, m.index);
    if (matAt >= 0 && /\bbool unlit = 1\b/.test(text.slice(matAt, m.index))) {
      entries.push({ path: shader, attrs: [
        'color3f inputs:diffuseColor.connect = None',
        'color3f inputs:diffuseColor = (0, 0, 0)',
        'float inputs:ior = 1',
        'float inputs:roughness = 1',
        'float inputs:metallic.connect = None',
        'float inputs:metallic = 0',
        'normal3f inputs:normal.connect = None',
        'float inputs:occlusion.connect = None',
      ] });
      continue;
    }
    // A value routed to emission must also cut the file's own emissive
    // connection, which would otherwise win over it (an emissive-textured
    // material — Rodin's "shaded" output, black base colour — kept drawing
    // its emission texture).
    entries.push({ path: shader, attrs: [
      ...(conn
        ? [`color3f inputs:emissiveColor.connect = ${conn[1]}`]
        : ['color3f inputs:emissiveColor.connect = None', `color3f inputs:emissiveColor = ${value[1]}`]),
      'color3f inputs:diffuseColor.connect = None',
      'color3f inputs:diffuseColor = (0, 0, 0)',
      'float inputs:ior = 1',
      'float inputs:roughness = 1',
      'float inputs:metallic.connect = None',
      'float inputs:metallic = 0',
      'normal3f inputs:normal.connect = None',
      'float inputs:occlusion.connect = None',
    ] });
  }
  return usdOvers([...entries, ...usdLightsOff(off)]);
}

// Render `file` with usdrecord at the default time ('static') and at each
// time; returns { label: pngPath }. `opts.camera`: a camera prim of the file
// to draw through instead of the session layer's VerifyCamera, and
// `opts.width` its picture's width (usdrecord takes the height from the
// camera's aspect) (CAMLIGHT).
async function usdrecordFrames(file, sessionText, times, tcps, dir, tag, extraArgs, opts = {}) {
  await fs.mkdir(dir, { recursive: true });
  const session = path.join(dir, `${tag}_session.usda`);
  await fs.writeFile(session, sessionText);
  const out = {};
  for (const t of ['static', ...times]) {
    const label = String(t);
    const sub = path.join(dir, `${tag}_${label}`);
    await fs.rm(sub, { recursive: true, force: true });
    await fs.mkdir(sub, { recursive: true });
    const args = ['--sessionLayer', session, '--camera', opts.camera ?? '/VerifyCameraRoot/VerifyCamera', '--imageWidth', String(opts.width ?? 256), ...extraArgs];
    // `--defaultTime` is a flag (store_true in OpenUSD's framesArgs.py): a
    // value after it is read as the USD file, and argparse then rejects the
    // real output path, so every static render failed. The CLI11 build wants
    // `--defaultTime=1` instead (USDRECORD_DEFAULT_TIME).
    if (t === 'static') args.push(USDRECORD_DEFAULT_TIME);
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

// `usdchecker --arkit` judges a file for AR, and for AR a USD file is a USDZ:
// its extra rules are what a package for AR Quick Look and RealityKit must
// meet. USDA and USDC outputs get the general checks, plain `usdchecker`.
async function validateUsd(file, format) {
  const tool = format === 'usdz' ? 'usdchecker --arkit' : 'usdchecker';
  const args = format === 'usdz' ? ['--arkit', file] : [file];
  // `variants`: what usdchecker reported for each variant selection of the file
  // (usdcheckerResults), judged by usdClipChecks where the file has several clips.
  try {
    const { stdout, stderr } = await run('usdchecker', args, { timeout: 60000 });
    return { tool, ok: true, messages: (stdout + stderr).split('\n').filter(Boolean).slice(-6), variants: usdcheckerResults(stdout + stderr) };
  } catch (e) {
    return {
      tool,
      ok: false,
      messages: String((e.stdout || '') + (e.stderr || '')).split('\n').filter(Boolean).slice(-12),
      variants: usdcheckerResults(String((e.stdout || '') + (e.stderr || ''))),
    };
  }
}

async function validate(file, format) {
  const out = [];
  if (format === 'glb' || format === 'gltf') out.push(await validateGltf(file));
  if (format.startsWith('usd') && TOOLS.usdchecker) out.push(await validateUsd(file, format));
  if (TOOLS.assimp && ['fbx', 'dae', '3mf', 'obj', 'ply', 'stl', 'glb', 'gltf'].includes(format)) {
    out.push(await validateAssimp(file));
  }
  return out;
}

// ─── uv sets ────────────────────────────────────────────────────────────────
//
// Every uv set the expected scene keeps is judged by some reader. The `uv`
// mode draws set 0, and the set selector's `uv1`–`uv3` draw sets 1–3
// (page/render.js), each through the target's three.js loader where that
// loader builds the set. Where it does not, SceneKit draws the set through
// `mappingChannel` (Collada and USD; it read ten sets of each,
// docs/superpowers/research/research-consumers.md Q7), and the three.js
// comparison is a skip citing the loader's lines. A set past the fourth has no
// reader here: three.js's materials address four, so not even its expected
// picture can be drawn.
const THREE_UV_SETS = { dae: 2, usdz: 2, usda: 2, usdc: 2, usd: 2 };
const THREE_UV_SKIP = {
  dae: 'three.js ColladaLoader builds TEXCOORD sets 0 and 1 only: it keys set n as TEXCOORDn (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/collada/ColladaParser.js:1338) and builds TEXCOORD and TEXCOORD1 alone (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/collada/ColladaComposer.js:1755-1763, :1776-1777); judged by SceneKit',
  usd: 'three.js USDLoader builds uv sets 0 and 1 only: the first texCoord primvar as `uv` and `primvars:st1` as `uv1` (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:1340-1398, :1478-1479, :1716-1722, :1752-1784); judged by SceneKit',
};
const LATE_UV_SKIP =
  'no reader here draws uv set 4 or later: three.js materials address sets 0-3 alone (scripts/mesh-verify/node_modules/three/build/three.module.js:7460-7468, :6904-6922), so not even the expected picture can be drawn';
// Which sets of a case three.js draws (`three`, modes), which SceneKit draws
// (`scenekit`, modes), and which three.js skips, with why (`skips`).
function uvSetPlan(c, info) {
  const sets = Math.min(info?.uvSets ?? 0, c.caps.uv_sets);
  const plan = { sets, three: [], scenekit: [], skips: {} };
  for (let set = 1; set < sets; set++) {
    const mode = `uv${set}`;
    if (set > 3) {
      plan.skips[mode] = LATE_UV_SKIP;
    } else if (set < (THREE_UV_SETS[c.to] ?? 4)) {
      plan.three.push(mode);
    } else {
      plan.skips[mode] = THREE_UV_SKIP[c.to === 'dae' ? 'dae' : 'usd'];
      plan.scenekit.push(mode);
    }
  }
  return plan;
}

// ─── points and lines (mesh-gaps item 10) ───────────────────────────────────
//
// WIRE (page/render.js). Points draw as THREE.Points (4 px, sizeAttenuation
// off) and lines as THREE.LineSegments (1 px) in every mode, with materials of
// the page's own, and the expected picture is GLTFLoader's. Each target's own
// three.js loader is used where it builds point or line objects, and where it
// cannot, that comparison is a skip citing the loader's source line, and
// another reader judges (research Q6, docs/superpowers/research/research-
// consumers.md L336-388):
//
//  - glb, gltf: GLTFLoader builds Points and LineSegments, LineLoop and Line
//    (GLTFLoader.js:3834-3880, :3442-3478): compared as any mode.
//  - dae: ColladaLoader builds `<lines>` as LineSegments (collada/
//    ColladaComposer.js:2560-2627): compared. Points never reach Collada: the
//    capability table strips them, and the expected scene keeps none.
//  - obj: OBJLoader builds `p` as Points and `l` as LineSegments (OBJLoader.js:
//    389-425): compared, but its lines carry no vertex colour (WIRE_OBJ_COLORS),
//    so the expected picture is drawn with its lines' colours left out
//    (`#linecolors`) and the colours are skipped, cited.
//  - ply: PLYLoader ignores the `edge` element (WIRE_PLY_EDGES): compared with
//    the expected picture drawn without its lines (`#lines`). Points come only
//    alone (the capability table drops them beside faces or edges), as an
//    unindexed geometry, which the page draws as THREE.Points.
//  - usda, usdz: USDLoader drops Points and BasisCurves (WIRE_USD_THREE):
//    compared with the expected picture drawn without either (`#wire`).
//
// Where three.js cannot draw them the other readers judge, on the bounding box
// and the centroid of the covered pixels of a picture of the points and lines
// alone, within WIRE_LIMIT of the frame (page/render.js, compareWire): a point
// or a line cannot be compared pixel for pixel across renderers (Storm draws a
// point at its world-space `widths`, SceneKit at its own size). Where the
// expected picture shows every point apart (as many blobs as points), the
// points are counted and the centroid is the mean of their centres (a footprint
// differs by renderer). A cloud whose points run together (fewer blobs than
// points: Stanford's drill, 4,223 points in 2 blobs) is judged on the box, the
// centroid of the convex hull of its covered pixels, and two content checks,
// every covered pixel within 1 px of the expected picture's and the covered 8 px
// cells within a cell of each other's (compareWire, denseChecks).
//  - usdrecord, for USD: the Mesh prims are deactivated, with the BasisCurves
//    (for the points) or the Points (for the lines), so Storm draws one kind
//    alone, against the expected `wire_cover_points` or `wire_cover_lines`
//    picture: each kind apart, as Storm's points are a pixel or so wide (the
//    `widths` litegen writes are 0.25 % of the scene's diagonal) where the
//    expected picture's are 4, so their place is judged and not their mass.
//    Its `cover` and `albedo` passes deactivate the Points and BasisCurves
//    instead, and compare with the expected picture drawn without them.
//  - SceneKit draws a Collada `<lines>` and a USD BasisCurves as lines (its
//    `wire` mode keeps the point and line elements alone, render.swift; its
//    other modes the triangle elements alone, so they compare with the
//    expected picture without points and lines): judged for dae and usd. It
//    drops USD Points, ignores PLY edges and drops OBJ `p` and `l` (the
//    SceneKit skips below), and a vertex-only PLY it draws as points.
// The expected picture `#wire`, `#lines` or `#linecolors` is the same file
// drawn as the reader would draw it (page/render.js, blind), so the triangle
// comparison stays whole.
const WIRE_LIMIT = 0.01;
// usdrecord draws the wire pictures at four times the frame's width. litegen
// writes a point's and a line's `widths` as 0.25 % of the scene's diagonal, a
// little over half a pixel on `wire` at 256 px, and Storm samples a pixel four
// times (coverage alpha 0/64/128/191/255): measured 2026-11-08, it drew 8 of
// the 9 grid points, one as two pixels of alpha 64 and the ninth none, so the
// centroid was 8.61 px off, where at 1024 px all nine are drawn, though with
// 53 covered pixels of the 1024 px frame (6 for each point) against the expected
// picture's 144 of 256 px (16 for each), so a centroid of the pixels was still
// 4.01 px off and the points are judged by their centres (compareWire). The picture is measured in the 256 px frame's
// units (page/render.js, compareWire).
const WIRE_USD_WIDTH = 1024;
const WIRE_EMPTY ='neither the expected picture nor this one draws a point or a line in this frame, so there is nothing to compare';
// How a wire picture differs from the expected one, for a failure's reason.
const wireApart = (r) => {
  if (!(r.got.pixels && r.want.pixels)) {
    return r.want.pixels
      ? `it draws no point or line where the expected picture has ${r.want.pixels} covered pixels`
      : `it draws ${r.got.pixels} covered pixels of points or lines where the expected picture has none`;
  }
  if (r.want.blobs !== r.got.blobs) return `it draws ${r.got.blobs} points where the expected picture has ${r.want.blobs}`;
  const parts = [];
  // The distance as the verdict took it: `ratio` is the largest unrounded one
  // over the frame (compareWire), except that a content check's own reason
  // raises it to 1, and `apart` is rounded to 0.01 px, so a box 2.56 to 2.565
  // px apart (past the limit) would read as 2.56 and say nothing.
  const far = r.why && r.apart ? Math.max(...r.apart) / 256 : r.ratio;
  if (r.apart && far > WIRE_LIMIT) {
    const what = r.want.method === 'box' ? `box (x0, y0, x1, y1) (cannot judge the ${r.want.cannotJudge || 'centroid: the covered pixels have no hull'})` :`box (x0, y0, x1, y1) and centroid${r.want.method === 'hull' ? " (of the covered pixels' convex hull)" : ''} (x, y)`;
    parts.push(`the covered pixels' ${what} are ${r.apart.join(', ')} px from the expected picture's, past ${WIRE_LIMIT * 256} px (1 % of the frame)`);
  }
  if (r.why) parts.push(r.why);
  return parts.join('; ');
};
const WIRE_OBJ_COLORS =`three.js OBJLoader keeps no vertex colour on a line: addPointGeometry reads one for each point (${THREE_LOADERS}/OBJLoader.js:400) and addLineGeometry never does (:406-425), so an OBJ \`l\` draws white where the expected line is coloured; the lines' place is judged in every mode; only their colour isn't, in albedo, and the unit tests judge it (obj_reads_p_and_l reads them from the \`v\` lines, wire_round_trips_through_obj writes and reads them back)`;
const WIRE_PLY_EDGES = `three.js PLYLoader ignores the edge element: handleElement reads \`vertex\` and \`face\` alone (${THREE_LOADERS}/PLYLoader.js:480-561), so a PLY file's lines have no three.js picture (SceneKit and assimp ignore PLY edges too, research Q6); judged by the unit tests (ply_edges_are_lines) and the matrix's PLY round trips`;
const WIRE_USD_THREE = `three.js USDLoader drops Points and BasisCurves: every prim but a Mesh falls into its generic branch and becomes an empty Object3D (${THREE_LOADERS}/usd/USDComposer.js:621-676, the Mesh branch at :650 and the generic one at :663-673); judged by usdrecord on the coverage's bounding box and centroid`;
const WIRE_SK_OBJ = "SceneKit drops OBJ `p` and `l` (research Q6, the SceneKit row, docs/superpowers/research/research-consumers.md:377, and the Model I/O row at :378: Model I/O reads neither, measured on macOS 26.6.2); the points and lines of an OBJ are judged by three.js";
const WIRE_SK_PLY = 'SceneKit ignores PLY edges (research Q6, the SceneKit row, docs/superpowers/research/research-consumers.md:377, measured on macOS 26.6.2; assimp and three.js PLYLoader ignore them too, WIRE_PLY_EDGES)';
const WIRE_SK_USD_POINTS = 'SceneKit drops USD Points: its importer gives them no node (research Q6, the SceneKit row, docs/superpowers/research/research-consumers.md:377, measured on macOS 26.6.2); judged by usdrecord';
// assimp refuses a point cloud: its PLY importer makes a vertex-only file a
// mesh of points with no faces, and its own validation rejects that.
const WIRE_ASSIMP_NO_FACES = "assimp refuses a PLY with no faces: its importer makes a vertex-only file a mesh of points with zero faces, which ValidateDataStructure rejects (\"Mesh contains no faces\", code/PostProcessing/ValidateDataStructure.cpp:L342-343 @v6.0.5; research Q6, the assimp row, docs/superpowers/research/research-consumers.md:382: that step is on in `assimp info`'s default preset, `-fd` replaces the preset); the file is judged by the other readers";
// Wire comparisons skipped with a citation: three.js's loaders (obj, ply, usd),
// SceneKit's importers and assimp's validation (a PLY with no faces). Counted
// per case and judge, reported beside the native checks' (`wireSkipped`).
const wireSkips = { three: 0, scenekit: 0, assimp: 0 };
function wireSkip(entry, judge, key, reason) {
  wireSkips[judge]++;
  if (judge === 'three') entry.results[key] = { static: { skipped: reason } };
  else (entry.scenekit ??= {})[key] = { skipped: reason };
}
// What the expected scene's points and lines ask of each reader, for a case:
// `three` (the expected picture's blind variant, and why), `usdrecord`
// (whether it draws them) and `scenekit` (the `wire` picture it can draw and
// the expected picture to compare it with, and the skips).
function wirePlan(c, info) {
  const points = info?.points ?? 0;
  const lines = info?.lines ?? 0;
  const plan = { points, lines, pointVertices: info?.pointVertices ?? 0, any: points + lines > 0, three: null, usdrecord: false, scenekit: { expected: null, skips: {} } };
  if (!plan.any) return plan;
  if (c.to === 'obj' && lines) plan.three = { blind: 'linecolors', skip: WIRE_OBJ_COLORS };
  else if (c.to === 'ply' && lines) plan.three = { blind: 'lines', skip: WIRE_PLY_EDGES };
  else if (USD_FORMATS[c.to]) plan.three = { blind: 'wire', skip: WIRE_USD_THREE };
  plan.usdrecord = !!USD_FORMATS[c.to];
  const sk = plan.scenekit;
  if (c.to === 'dae') {
    if (lines) sk.expected = 'wire_cover_lines';
  } else if (USD_FORMATS[c.to]) {
    if (lines) sk.expected = 'wire_cover_lines';
    if (points) sk.skips['wire:points'] = WIRE_SK_USD_POINTS;
  } else if (c.to === 'ply') {
    if (lines) sk.skips['wire:lines'] = WIRE_SK_PLY;
    else sk.expected = 'wire_cover_points';
  } else if (c.to === 'obj') sk.skips.wire = WIRE_SK_OBJ;
  return plan;
}

// ─── render ─────────────────────────────────────────────────────────────────

function modesFor(caps) {
  const modes = ['geom', 'albedo'];
  if (caps.normals) modes.push('normals');
  if (caps.uv_sets > 0) modes.push('uv');
  return modes;
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
    // has a clip. The per-set uv modes only for the sets the file has.
    const job = {
      key: `${name}|${key}`,
      url: `${base}/m/${rel}`,
      format: 'glb',
      camera: fx.camera,
      // The camera and light modes are drawn only where the file has a camera
      // or lights; `lights_each` is one `light:<node>` picture per light
      // (CAMLIGHT).
      // The wire pictures (WIRE) only where the file has points or lines.
      modes: ['geom', 'albedo', 'normals', 'uv', 'uv1', 'uv2', 'uv3', 'cover', 'lit', 'lit_flat', 'lit_coat', 'camera', 'camera_cover', 'lit_lights', 'lights_each', 'wire_cover_points', 'wire_cover_lines'],
      presentUvSetsOnly: true,
      // Its translucency and texture alpha (ALPHA_ORACLE_SKIP).
      alphaStats: true,
      times: fx.times,
      clipName: fx.animations?.[0],
    };
    const r = await page.evaluate((j) => window.renderExpected(j), job);
    const images = {};
    for (const [mode, byT] of Object.entries(r.images)) {
      images[mode] = {};
      for (const [t, url] of Object.entries(byT)) {
        images[mode][t] = await saveDataUrl(url, path.join(outDir, 'img', name, 'expected', key, `${mode.replace(/:/g, '_')}_${t}.png`));
      }
    }
    fxReport.expected[key] = { path: rel, images, stats: r.stats, layers: r.stats.layers, validation: await validate(path.join(matrixDir, rel), 'glb') };
    // The numeric oracle's calibration (CAMLIGHT): assimp's glTF importer on
    // the expected GLB itself must agree with it, after the adjustments
    // camlight-oracle.mjs cites.
    if (TOOLS.assimp && (r.stats.cameras?.length || r.stats.lights?.length)) {
      const o = await oracleFile(path.join(matrixDir, rel), path.join(matrixDir, rel), path.join(outDir, 'assjson', name, 'expected', `${key}.assjson`), r.stats.clips?.length ? fx.times || [] : []);
      fxReport.expected[key].camlightOracle = { passed: o.passed, failed: o.failed, skipped: o.skipped, ...(o.error ? { error: o.error } : {}), checks: o.checks };
      camlightTotals.expectedFiles++;
      if (o.error || o.failed) {
        camlightTotals.expectedFailed++;
        const said = o.error || o.checks.filter((x) => x.verdict === 'fail').map((x) => `${x.subject} / ${x.field}`).join(', ');
        process.stdout.write(`FAIL expected ${name} ${key}: the camlight oracle disagrees with the expected file itself: ${said}\n`);
      }
    }
    // What the lit judges can see of this file's layers (litPlan).
    if (r.stats.layers?.length) {
      const vis = await page.evaluate((j) => window.layerVisibility(j), { key: job.key, url: job.url, camera: fx.camera, modes: ['lit', 'lit_coat'] });
      litVisibility.set(job.key, vis);
      fxReport.expected[key].litSeen = vis;
    }
    process.stdout.write(`expected ${name} ${key}${r.stats.animated ? ' (animated)' : ''}\n`);
  }
}

// Every way a run can fail counts here: a case, a native file our reading of
// which differs from its own loader's, an import our reader refused, and an
// expected file the camlight oracle disagrees with (its calibration).
let failures = camlightTotals.expectedFailed;
let nativeFailed = 0;
let nativeSkipped = 0;

// Third-party originals: their own loader's picture against our reading.
for (const [name, fx] of Object.entries(manifest.fixtures)) {
  if (!fx.native || (onlyFixtures && !onlyFixtures.includes(name))) continue;
  // An original whose own loader is measured not to decode it (ASSET_LIMITS,
  // nativeCheck): the native check is skipped with the citation, and every
  // other check on the file still runs.
  const nativeLimit = await assetLimit(fx, 'nativeCheck', 'three');
  if (nativeLimit) {
    report.fixtures[name].nativeCheck = { skipped: nativeLimit, reasons: [] };
    nativeSkipped++;
    process.stdout.write(`skip native ${name}  ${nativeLimit.slice(0, 160)}…\n`);
    continue;
  }
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
  // Our side also draws the point and line pictures (`wire_cover_*`, kept in
  // the cache for the wire judges), where the reading has any.
  const pairOf = (blind) =>
    page.evaluate(
      ([a, b, k]) => window.renderPair(a, b, k),
      [
        { url: `${base}/m/${fx.native.path}`, format: fx.native.format, files: [], modes: pairModes, times: fx.times || [], clipName: fx.animations?.[0] },
        { url: `${base}/m/${fx.reference}`, format: 'glb', files: [], modes: [...pairModes, 'wire_cover_points', 'wire_cover_lines'], presentUvSetsOnly: true, blind, times: fx.times || [], clipName: fx.animations?.[0] },
        cacheKey,
      ],
    );
  let r = await pairOf();
  const check = { reasons: [] };
  // A KTX2 original (KHR_texture_basisu, drawn through KTX2Loader): the GPU
  // format(s) this browser transcoded it to go in the report, and it is drawn
  // against ITSELF, loaded twice. That calibration control must be 0.00 %; the
  // albedo limit against our reading is KTX2_NATIVE_ALBEDO.
  const ktx2 = r.nativeStats?.ktx2;
  if (ktx2) {
    const spec = { url: `${base}/m/${fx.native.path}`, format: fx.native.format, files: [], modes: ['geom', 'albedo'], times: [] };
    const again = await page.evaluate(([a, b]) => window.renderPair(a, b, null), [spec, spec]);
    check.ktx2 = { ...ktx2, self: Object.fromEntries(Object.entries(again.results || {}).map(([mode, x]) => [mode, x.ratio])) };
    if (again.error) check.reasons.push(`KTX2Loader against itself: ${again.error}`);
    for (const [mode, ratio] of Object.entries(check.ktx2.self)) {
      if (ratio !== 0) check.reasons.push(`KTX2Loader against itself: ${mode} ${(ratio * 100).toFixed(2)}% apart`);
    }
    // An empty self-comparison proves nothing: both modes must have a result.
    for (const mode of spec.modes) {
      if (check.ktx2.self[mode] === undefined) check.reasons.push(`KTX2Loader against itself: no ${mode} picture`);
    }
    // The same pair with every texture sampled from its base level alone
    // (render.js, baseLevelOnly): the pictures then differ by the content and
    // the GPU format alone, not by the original's mip levels against the ones
    // the GPU builds from our PNG. Judged at KTX2_NATIVE_LEVEL0.
    const level0Pair = (tolerance) =>
      page.evaluate(([a, b, tol]) => window.renderPair(a, b, null, tol), [
        { ...spec, modes: ['albedo'], level0: true },
        { url: `${base}/m/${fx.reference}`, format: 'glb', files: [], modes: ['albedo'], times: [], level0: true },
        tolerance,
      ]);
    const to = path.join(outDir, 'img', name, 'native');
    const keep = async (label, pair, limit) => {
      const res = pair.results?.albedo;
      if (pair.error || !res) {
        check.reasons.push(`albedo from level 0 alone (${label}): ${pair.error || 'no picture'}`);
        return;
      }
      check.ktx2[label] = {
        ratio: res.ratio,
        differ: res.differ,
        covered: res.covered,
        maxDelta: res.maxDelta,
        native: await saveDataUrl(res.native, path.join(to, `albedo_${label}_native.png`)),
        ours: await saveDataUrl(res.ours, path.join(to, `albedo_${label}_ours.png`)),
        diff: await saveDataUrl(res.diff, path.join(to, `albedo_${label}_diff.png`)),
      };
      if (res.ratio > limit) check.reasons.push(`albedo from level 0 alone (${label}): ${(res.ratio * 100).toFixed(3)}% differ from the native rendering (limit ${(limit * 100).toFixed(3)}%)`);
    };
    await keep('level0', await level0Pair(), KTX2_NATIVE_LEVEL0);
    // And through KTX2Loader's RGBA32 fallback (render.js, ktx2Rgba32): the
    // transcoder's own RGBA8, which litegen's decoder reproduces byte for
    // byte, so any pixel past KTX2_RGBA32_TOLERANCE is a decoding fault.
    await page.evaluate(() => window.setKtx2Mode('rgba32'));
    try {
      await keep('rgba32', await level0Pair(KTX2_RGBA32_TOLERANCE), 0);
    } finally {
      await page.evaluate(() => window.setKtx2Mode('gpu'));
    }
  }
  // What the original's own loaders can draw of the reading's points and
  // lines (WIRE). Where three.js builds fewer than our reading holds, our side
  // is drawn without them too, with the citation.
  const wireNative = wirePlan({ to: fx.native.format }, r.ourStats);
  if (wireNative.three) {
    r = await pairOf(wireNative.three.blind);
    check.wire = { skipped: wireNative.three.skip };
    wireSkips.three++;
  }
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
      // Its up too: render.swift aims the camera with +Y up unless told, and
      // a Z-up stage's up is +Z (a Z-up original drawn rolled read 58.39 %
      // apart, Blender's export of `staged`).
      skCamera = { ...r.ourCamera, position: toNative(r.ourCamera.position), target: toNative(r.ourCamera.target), up: toNative([0, 1, 0], false), near: r.ourCamera.near / mpu, far: r.ourCamera.far / mpu };
    }
    // The triangles are compared with our reading drawn without its points
    // and lines (`#wire`); the points and lines SceneKit draws, as `wire`; the
    // ones it does not, skipped with the citation.
    const skKey = wireNative.any ? `${cacheKey}#wire` : cacheKey;
    for (const [mode, reason] of Object.entries(wireNative.scenekit.skips)) {
      check.scenekit[mode] = { skipped: reason };
      wireSkips.scenekit++;
    }
    for (const mode of ['silhouette', 'albedo', ...(wireNative.scenekit.expected ? ['wire'] : [])]) {
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
      const compareTo = (query) =>
        mode === 'wire'
          ? page.evaluate((j) => window.compareWire(j), { url: `${base}/out/${path.relative(outDir, png)}${query}`, expectedKey: cacheKey, mode: wireNative.scenekit.expected, t: 'static', blobs: wireNative.scenekit.expected === 'wire_cover_points' ? wireNative.pointVertices : 0 })
          : page.evaluate(
              ([url, k, m]) => window.compareImage(url, k, m, 'static'),
              [`${base}/out/${path.relative(outDir, png)}${query}`, skKey, mode],
            );
      let cmp = await compareTo('');
      // An empty frame where a picture is expected: rendered once more, as in
      // the case checks below.
      let retried = false;
      if (!cmp.error && cmp.gotDrawn === 0 && cmp.wantDrawn > 0) {
        const again = await scenekitRender(path.join(matrixDir, fx.native.path), skCamera, mode, null, png);
        if (again && !again.error) {
          cmp = await compareTo('?retry=1');
          retried = true;
        }
      }
      const item = { image: path.relative(outDir, png), ratio: cmp.ratio, error: cmp.error, ...(retried ? { retried: 'SceneKit drew an empty frame; rendered again' } : {}) };
      let limit = mode === 'wire' ? WIRE_LIMIT : mode === 'silhouette' ? 0.01 : 0.02;
      let diff = mode === 'wire' ? cmp.overlay : cmp.diff;
      if (mode === 'wire') Object.assign(item, { apart: cmp.apart, want: cmp.want, got: cmp.got });
      // Albedo is judged on 4x4 block means (Ruling A38).
      if (mode === 'albedo' && !cmp.error) {
        const v = await scenekitAlbedoVerdict(`${base}/out/${path.relative(outDir, png)}${retried ? '?retry=1' : ''}`, skKey, 'static', cmp);
        Object.assign(item, { ratio: v.ratio, pixelRatio: v.pixelRatio, blocks: v.blocks, judge: v.judge });
        limit = v.limit;
        diff = v.diff;
      }
      if (diff) item.diff = await saveDataUrl(diff, path.join(dir, `scenekit_${mode}_native_diff.png`));
      check.scenekit[mode] = item;
      if (!cmp.error && cmp.empty) {
        item.skipped = WIRE_EMPTY;
      } else if (cmp.error || item.ratio > limit) {
        check.reasons.push(`SceneKit ${mode}: ${cmp.error || (mode === 'wire' ? wireApart(cmp) : (item.ratio * 100).toFixed(2) + (item.judge === 'blocks' ? '% of 4x4 blocks' : '%') + ' differ from the native file')}`);
      }
      if (mode === 'albedo' && !cmp.error) scenekitAlbedo = item.ratio <= limit;
      if (mode === 'silhouette' && !cmp.error) scenekitSilhouette = item.ratio <= limit;
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
    if (res.ratio > (mode === 'albedo' && ktx2 ? KTX2_NATIVE_ALBEDO : 0.05)) {
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
  // The lit pass (Ruling A35): a glTF original with a layer on either side,
  // through one camera, at THRESHOLD.lit and the lit tolerance; the layer
  // sets must match too (nativeLitRotations is the one cited excuse).
  if (GLTF_NATIVE[fx.native.format] && !r.error && (r.nativeStats?.layers?.length || r.ourStats?.layers?.length)) {
    const litJob = {
      native: `${base}/m/${fx.native.path}`,
      format: fx.native.format,
      ours: `${base}/m/${fx.reference}`,
      camera: fx.camera,
      restoreRotations: true,
      // A KTX2 original is drawn from level 0 on both sides (baseLevelOnly).
      level0: !!ktx2,
    };
    // A KTX2 original is judged here through KTX2Loader's RGBA32 fallback, the
    // transcoder's exact RGBA8 (the GPU format this browser picks is block
    // compression, which moves lit pixels by itself); the picture through the
    // GPU format is recorded beside it, not judged (`check.ktx2.litGpu`).
    if (ktx2) {
      const gpu = await page.evaluate((j) => window.renderNativeLit(j), litJob);
      if (!gpu.error) check.ktx2.litGpu = { ratio: gpu.ratio, differ: gpu.differ, covered: gpu.covered, maxDelta: gpu.maxDelta };
      await page.evaluate(() => window.setKtx2Mode('rgba32'));
    }
    let lit;
    try {
      lit = await page.evaluate((j) => window.renderNativeLit(j), litJob);
    } finally {
      if (ktx2) await page.evaluate(() => window.setKtx2Mode('gpu'));
    }
    if (lit.error) {
      check.lit = { error: lit.error };
      check.reasons.push(`lit: ${lit.error}`);
    } else {
      check.lit = {
        ratio: lit.ratio,
        differ: lit.differ,
        covered: lit.covered,
        maxDelta: lit.maxDelta,
        layers: lit.layers,
        native: await saveDataUrl(lit.native, path.join(dir, 'lit_native.png')),
        ours: await saveDataUrl(lit.ours, path.join(dir, 'lit_ours.png')),
        diff: await saveDataUrl(lit.diff, path.join(dir, 'lit_diff.png')),
        ...(lit.restored ? { restored: lit.restored } : {}),
      };
      const sorted = (l) => [...(l || [])].sort().join(',');
      if (sorted(lit.layers.native) !== sorted(lit.layers.ours)) {
        check.reasons.push(`lit: the layers differ: the original [${lit.layers.native}], our reading [${lit.layers.ours}]`);
      }
      if (lit.ratio > THRESHOLD.lit) {
        const closed = lit.restored && !lit.restored.error && lit.restored.rotations > 0 && lit.restored.ratio <= THRESHOLD.lit;
        if (closed && lit.restored.largest <= LIT_ROTATION_BOUND) {
          check.lit.excused = nativeLitRotations(lit, lit.restored);
          process.stdout.write(`note native ${name}: lit ${lit.differ} pixel(s) apart, 0 with the original's rotations put back (moved by ${lit.restored.largest} at most)\n`);
        } else {
          const rotated = closed ? `; the original's rotations close it, but one differs from ours by ${lit.restored.largest}, past the ${LIT_ROTATION_BOUND} a renormalization explains` : '';
          check.reasons.push(`lit: ${(lit.ratio * 100).toFixed(2)}% of pixels differ from the native rendering (one camera, the lit tolerance; max delta ${lit.maxDelta})${rotated}`);
        }
      }
    }
  }
  // The camera and light passes (CAMLIGHT, item 5's Ruling R24; the spec:
  // "The native check runs in camera and lit mode"): a glTF original with a
  // camera or a light on either side, as GLTFLoader builds them, is drawn
  // with our reading of it in `camera`, each through its own camera (ours by
  // the original's first camera's node name), and in `lit_lights`, at rest
  // and at the sample times, both judged at 0.00 %; the two sides' cameras
  // and lights must match by name and kind. As in the lit pass, the one
  // excuse is a repeat with the original's rotations put back that closes
  // every difference, none moved past LIT_ROTATION_BOUND.
  if (GLTF_NATIVE[fx.native.format] && !r.error) {
    const has = (k) => !!(r.nativeStats?.[k]?.length || r.ourStats?.[k]?.length);
    const modes = [...(has('cameras') ? ['camera'] : []), ...(has('lights') ? ['lit_lights'] : [])];
    if (modes.length) {
      const cl = await page.evaluate((j) => window.renderNativeCamlight(j), {
        native: `${base}/m/${fx.native.path}`,
        format: fx.native.format,
        ours: `${base}/m/${fx.reference}`,
        camera: fx.camera,
        modes,
        times: fx.times || [],
        clipName: fx.animations?.[0],
        restoreRotations: true,
      });
      if (cl.error) {
        check.camlight = { error: cl.error };
        check.reasons.push(`camlight: ${cl.error}`);
      } else {
        check.camlight = { cameraName: cl.cameraName, cameras: cl.cameras, lights: cl.lights, ...(cl.restored ? { restored: cl.restored } : {}) };
        const list = (xs) => [...(xs || [])].map((x) => `${x.name}:${x.kind}`).sort().join(', ');
        for (const k of ['cameras', 'lights']) {
          if (list(cl[k].native) !== list(cl[k].ours)) check.reasons.push(`${k}: the original has [${list(cl[k].native)}], our reading [${list(cl[k].ours)}]`);
        }
        const bad = [];
        for (const [mode, byT] of Object.entries(cl.results)) {
          check.camlight[mode] = {};
          for (const [t, res] of Object.entries(byT)) {
            const file = `${mode}_${t}`;
            const item = res.error
              ? { error: res.error }
              : {
                  ratio: res.ratio,
                  differ: res.differ,
                  maxDelta: res.maxDelta,
                  native: await saveDataUrl(res.native, path.join(dir, `${file}_native.png`)),
                  ours: await saveDataUrl(res.ours, path.join(dir, `${file}_ours.png`)),
                  ...(res.diff ? { diff: await saveDataUrl(res.diff, path.join(dir, `${file}_diff.png`)) } : {}),
                };
            check.camlight[mode][t] = item;
            if (res.error || res.ratio > 0) bad.push(`${mode}@${t}: ${res.error ?? `${(res.ratio * 100).toFixed(2)}% of pixels differ from the native rendering (max delta ${res.maxDelta})`}`);
          }
        }
        if (bad.length) {
          const closed = cl.restored && !cl.restored.error && cl.restored.rotations > 0 && cl.restored.differ === 0;
          if (closed && cl.restored.largest <= LIT_ROTATION_BOUND) {
            check.camlight.excused = `${bad.length} frame(s) apart (${bad[0]}), 0 with the original's ${cl.restored.rotations} rotation(s) put back into our reading, none moved by more than ${cl.restored.largest} (bound ${LIT_ROTATION_BOUND}): the rotation renormalization the lit pass excuses and cites (nativeLitRotations)`;
            process.stdout.write(`note native ${name}: camlight ${bad.length} frame(s) apart, 0 with the original's rotations put back\n`);
          } else {
            const rotated = closed ? `; the original's rotations close it, but one differs from ours by ${cl.restored.largest}, past the ${LIT_ROTATION_BOUND} a renormalization explains` : '';
            for (const b of bad.slice(0, 4)) check.reasons.push(`camlight ${b}${rotated}`);
          }
        }
      }
    }
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
  if (check.reasons.length) {
    nativeFailed++;
    failures++;
  }
}

// ─── dae-keep (mesh-gaps item 8, Ruling DM15) ──────────────────────────────
//
// `mesh_matrix --dae-keep` adds, for every direction into Collada, a case whose
// file is written with `DaeMorphs::Keep` (`to: 'dae-keep'` in the manifest).
// Every judge reads it as Collada, SceneKit's pictures at rest and at every
// sample time included, with three exceptions (dae-keep.mjs):
//   - three.js ColladaLoader and assimp must REFUSE a file that holds a
//     `<morph>` (decided per file); if either reads it, the case fails with
//     "documented reader limitation no longer holds";
//   - xmllint judges every dae-keep file against the COLLADA 1.4.1 schema,
//     fetched into a cache outside the repo, its sha-256 pinned, its xml.xsd
//     import resolved offline through a catalog.
// SceneKit alone judges the geometry of a file that holds a <morph>, so its
// silhouette must be judged at every sample time (DM18, keepSilhouetteVerdict),
// and a file for an expected scene with targets must hold one (keepMissingMorph);
// except that animated frames all skipped for SceneKit's shared-clips limitation
// pass as "unjudged, cited" and are counted apart (DM21, summary.daeKeep).
// Its uv frames are judged against SceneKit's own picture of the plain Collada
// file at rest (DM20, keepUvJudge).
// `animatedUnjudged`: the cases whose animated SceneKit frames were all skipped
// for the shared-clip limitation, passing with the rest frame alone (DM21).
const daeKeepTotals = { cases: 0, morphFiles: 0, threeRefused: 0, assimpRefused: 0, animatedUnjudged: 0, xmllint: { checked: 0, valid: 0, invalid: 0, schemaBroken: 0, skipped: 0, refused: 0 } };
let daeSchemaPrepared = null;
async function daeSchema() {
  if (daeSchemaPrepared) return daeSchemaPrepared;
  daeSchemaPrepared = (await haveXmllint()) ? await prepareSchema() : { status: 'skipped', reason: 'xmllint is not installed' };
  // The report names the cache from `~`, never from the home folder.
  daeKeepTotals.schema = schemaForReport(daeSchemaPrepared);
  if (daeSchemaPrepared.status !== 'ready') {
    process.stdout.write(`xmllint pass ${daeSchemaPrepared.status}: ${daeSchemaPrepared.reason}\n`);
  }
  return daeSchemaPrepared;
}
// xmllint's record for one dae-keep file, in the shape `validate` returns.
async function xmllintRecord(file) {
  const x = daeKeepTotals.xmllint;
  const schema = await daeSchema();
  // A reason goes into each case's `validation`, so it names the cache from `~`
  // as `summary.daeKeep.schema` does, never by the home folder.
  if (schema.status === 'skipped') {
    x.skipped++;
    return { tool: 'xmllint', ok: true, skipped: schemaForReport(schema).reason };
  }
  if (schema.status === 'refused') {
    x.refused++;
    return { tool: 'xmllint', ok: false, messages: [`the COLLADA schema was refused: ${schemaForReport(schema).reason}`] };
  }
  const r = await xmllintFile(file, schema);
  x.checked++;
  if (r.kind === 'valid') x.valid++;
  else if (r.kind === 'invalid') x.invalid++;
  else x.schemaBroken++;
  const messages = r.kind === 'schema' ? [`the schema did not compile, which says nothing of the file: ${r.messages.join(' / ')}`] : r.messages;
  return { tool: 'xmllint', ok: r.ok, kind: r.kind, errors: r.errors, ...(messages.length ? { messages } : {}) };
}
// SceneKit's rest uv picture of a dae-keep file against SceneKit's own picture
// of the plain Collada file (Ruling DM20, keepUvJudge): both PNGs read here, and
// compared as the page compares a picture with an expected one (uv tolerance 24).
// null when either cannot be read, and the expected picture judges instead.
let pixelReader = null;
async function readPng(file) {
  try {
    pixelReader ??= await loadPixelReader();
    return pixelReader(await fs.readFile(file));
  } catch {
    return null;
  }
}
async function keepUvAgainstPlain(plainPng, keepPng) {
  const [want, got] = [await readPng(plainPng), await readPng(keepPng)];
  if (!want || !got || want.width !== got.width || want.height !== got.height) return null;
  return compareNearPixels(want.rgba, got.rgba, 24, got.width, got.height);
}

// ─── USDC, several clips in USD, unlit in USD (mesh-gaps item 6) ─────────────
//
// `usdc` is a USD format like `usda`: every USD judge above reads it
// (USD_FORMATS), and what is new is judged by usdItem6Checks, usdcPairChecks
// and their helpers below (the rules, with their tests, are usd-variants.mjs):
//
//  - usdcat reads each USD file with no warning, and prints the USDC of a source
//    as the same text as its USDA; usdrecord draws the two alike, pixel for
//    pixel (usdcPairChecks).
//  - A file with several clips (an `Animation` variant set: `usda`, `usdc` and
//    mesh_matrix --usd-variants' `usdz-all` and `usdz-crate`; `usdz-crate-first`
//    keeps the first clip alone, so it has no variant set to select): usdrecord draws
//    each variant selected through a session layer, at rest and at every sample
//    time of that clip, against that clip's expected picture; usdchecker reports
//    every variant; three.js lists every clip; SceneKit draws the authored
//    selection (its SceneKit passes) and cites why it draws no other
//    (usdClipChecks).
//  - Unlit (the `unlit` fixture, written directly or from its GLB): the unlit
//    meshes show the same pixels at two light levels, equal to the base colour,
//    where the lit control in the same frame changes, by usdrecord, SceneKit and
//    three.js (usdUnlitChecks).
const USD_ITEM6 = {
  active: false,
  usdcat: { files: 0, clean: 0 },
  usdcVsUsda: { pairs: 0, textEqual: 0, textLines: 0, pictures: 0, picturesIdentical: 0, maxDelta: 0 },
  clips: { files: 0, variants: 0, usdcheckerVariants: 0, usdrecordFrames: 0, threeListed: 0, sceneKitSkipped: 0, sceneKitSelected: 0 },
  unlit: { cases: 0, judged: { usdrecord: 0, scenekit: 0, three: 0 }, baseRecordedOnly: { scenekit: 0, three: 0 } },
  threeCrateEmptyArraySkips: 0,
};
// SceneKit draws the variant a USD file selects and no other: its variant
// support is "partial", and nothing in this harness switches a variant for it.
const SCENEKIT_VARIANT_SKIP =
  'SceneKit reads no USD variant selection (its variant support is rated "partial", docs/superpowers/research/research-consumers.md:77) and plays every SkelAnimation prim of a multi-clip file at once (measured, the note below), so only the first clip, which the harness selects by keeping its player (render.swift playClip), is drawn by SceneKit, in the SceneKit passes of this case; each variant is drawn by usdrecord through a session layer (research-consumers.md:71, usdrecord.py:L141-144, L271-296 at v26.11.1)';
// What SceneKit does with a USD file that holds several clips (measured
// 2026-11-10 on `clips`: usda, usdc, usdz-all and usdz-crate alike). Its USD
// importer keys one player per SkelAnimation prim by its path
// (`/clips/leg/walk`, `/clips/leg/run`, `/clips/leg/wave`; the rigid xformOps are
// one `transform` player on the prop) and plays all of them at once, whatever
// `skel:animationSource` binds and whichever variant is selected (also with the
// variant set flattened away): left alone, its silhouette is 25.61 %, 43.13 %,
// 47.01 %, 32.44 % and 35.04 % apart from the selected clip's expected picture
// at the five sample times after rest (limit 1 %), nearest the file's last prim
// (wave: 2-19 % from SceneKit's own picture of a file with that prim alone),
// never the bound one (walk: 17-51 %). usdrecord draws the same file as the
// selected clip at every time. So the harness tells SceneKit which player to
// keep (`playClip`, render.swift): its picture of the first clip, the one the
// file selects, is then judged like any other, and the other variants are cited
// skips (SCENEKIT_VARIANT_SKIP). SceneKit's variant support is rated "partial"
// (docs/superpowers/research/research-consumers.md:77).
// And it keys a USD xformOp animation's samples over the whole stage's time
// range, not the span they were authored in (measured 2026-11-10, macOS 26.6.2,
// `clips` as a USDA, walk 1 s, run 0.6 s, wave 2 s, stage 0-60 at 30 per second
// and the prop's translate samples at 0 and 30): the prop's `transform` key
// lasts 2.0 s against the walk player's 1.0 s, and the prop's row at scene time
// 2 s equals usdrecord's at 1 s. With the other players removed the silhouette
// was still 3.61 %, 7.53 %, 11.82 %, 16.48 % and 19.74 % apart at the five walk
// times, the leg right and the prop slow (a USDZ of one clip, whose stage ends
// with its clip, shows no such lag). So render.swift plays the rigid keys at
// stage span / clip span when `playClip` is set, which puts the prop where
// usdrecord has it at the same time (prop row 150.3 against 150.4 at 1 s).
// Storm's dome light adds specular to a surface that has none: the lobe is
// `prefilter * (F * brdf.x + brdf.y)` (previewSurface.glslfx:327, Apple USD Tools
// 0.25.2, /usr/lib/usd/usd/usdShaders/resources/shaders/), and `brdf.y`, the
// bias of the split-sum environment BRDF, survives a black F0 and scales with
// the dome. Measured 2026-11-10 on the unlit recipe (diffuse black, metallic 1,
// roughness 1): the same residual (mean 0.746/255, max 6, 14.14 % of pixels past
// 1/255) with metallic 1, with `useSpecularWorkflow` and a black specular colour,
// and with metallic 0 and ior 1, and none under a distant light alone (0 of
// 5,388 pixels differ under a normalised sun of 0.5 and 1.5, with the lit
// control moving 98.5 % of its pixels; a first measure with an unnormalised sun
// drew no light at all and showed nothing: UNLIT_LEVELS). So usdrecord's
// invariance is judged under the distant light and the dome's residual is
// recorded, not judged.
const STORM_DOME_RESIDUAL =
  "Storm's dome light adds `prefilter * (F * brdf.x + brdf.y)` (/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:327, Apple USD Tools 0.25.2), and brdf.y survives a black specular colour and scales with the dome: a residual of the renderer, a measurement here and not a judge (Ruling U22); the invariance is judged under a distant light alone, at the 1/255 tolerance";
// three.js's USDCParser reads a crate's empty array as an array at offset 0, so
// a crate that holds one is not three.js's to judge (the same data is judged in
// the file's USDA twin by three.js, and in the crate by usdrecord).
const THREE_CRATE_EMPTY_ARRAY = (names) =>
  `three.js's USDCParser cannot read a crate that holds an empty array: its _readValue seeks to the payload of any array that is not inlined and has no payload-0 case (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDCParser.js:1087-1102, r183), where OpenUSD packs an empty array as payload 0 and unpacks it as empty (pxr/usd/sdf/crateFile.cpp:1775-1777 and :1805-1808 at v26.11.1). Measured 2026-11-10: a crate Apple USD Tools 0.25.2's own usdcat wrote of \`uniform token[] joints = []\` fails in USDCParser.parseData with "Offset is outside the bounds of the DataView", one of \`uniform matrix4d[] m = []\` with "Invalid typed array length", and \`token[] names = ["x", "y"]\` parses. This file holds an empty array on: ${names.join(', ')}. usdrecord judges the crate, and three.js judges the same data in the file's USDA twin`;
async function threeCrateEmptyArray(file) {
  if (!TOOLS.usdcat) return null;
  const found = crateEmptyArrays(await usdRootText(file).catch(() => ''));
  return found.count ? found.names : null;
}
const THREE_USD_BASE_SKIP =
  'three.js USDLoader decodes linear UsdPreviewSurface base and emissive colours as sRGB (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:2512-2520 and :2541-2562), so the base colour is recorded, not judged; the base colour is judged by usdrecord, and the light invariance by every renderer';
// The two light levels of each renderer, tuned so that nothing clips and the
// lit control clearly changes (units differ: usdrecord's dome and distant
// light intensities, SceneKit's lumens, three.js's ambient and directional).
const UNLIT_LEVELS = {
  // usdrecord is judged under a distant light alone (the direct term; Storm's
  // dome light adds a residual to a surface with no specular colour, measured
  // under `domeResidual` and not judged: STORM_DOME_RESIDUAL). The sun is
  // normalised (`inputs:normalize = 1`): an unnormalised one of intensity 1
  // to 10 draws the lit control black (mean red 0.0 of 255), 100 gives 2.2,
  // 1000 gives 18.3, and a first run with 0.5 and 1.5 that way found no lit
  // pixel move at all, so it judged nothing (measured 2026-11-10, Apple USD
  // Tools 0.25.2, the `unlit` fixture's lit group alone); normalised, 0.5
  // gives 59.6 and 1.5 gives 102.0.
  usdrecord: [{ sun: 0.5 }, { sun: 1.5 }],
  usdrecordDome: [0.15, 0.45],
  scenekit: [150, 400],
  three: [
    { ambient: 0.4, directional: 0.8 },
    { ambient: 1.0, directional: 2.0 },
  ],
};
// The expected clip pictures already drawn (fixture|expected#clip:<name>).
const clipExpectedDrawn = new Set();
for (const raw of manifest.cases) {
  // `dae-keep` reads as Collada for every judge below (caseId).
  // `usdz-all`, `usdz-crate` and `usdz-crate-first` (mesh_matrix
  // --usd-variants, item 6) read as USDZ, with the variant named in `variant` (usd-variants.mjs).
  const c = raw.to === DAE_KEEP ? { ...raw, to: 'dae', variant: DAE_KEEP } : (usdVariantCase(raw) ?? raw);
  if (onlyFixtures && !onlyFixtures.includes(c.fixture)) continue;
  if (onlyFormats && !(onlyFormats.includes(c.to) && (c.from === 'scene' || c.from === 'single' || onlyFormats.includes(c.from)))) continue;
  const fx = manifest.fixtures[c.fixture];
  const entry = { ...c, to: raw.to, verdict: 'pass', reasons: [], results: {}, validation: [] };
  report.cases.push(entry);
  if (c.variant === DAE_KEEP) daeKeepTotals.cases++;
  // Refused, and rightly, only when the conversion failed the way the
  // expected scene does (Apple's skinned USDZ has no area at rest, so there
  // is no 3MF of it). Any other failure beside an expected one is a failure.
  // A scene of points or lines alone, for a format that holds neither, is one
  // refusal in two sentences: the writer names what it cannot hold
  // ("unsupported mesh feature: stl holds triangles only; this scene has 9
  // points and no triangles", scene_matrix_tests.rs, points_to_stl_is_named:
  // "refused by name, as `Unsupported`, not as an empty mesh"), and the expected
  // scene, reduced to what the format holds, is empty (`MeshError::Empty`,
  // mod.rs:432). The same holds for a conversion *from* such a format: the
  // generator wrote no source of it ("no stl source was written"), as the
  // source's own scene is empty too.
  // The format named is the case's own: the target's for a write refusal, the
  // source's for a missing source.
  const namedRefusal =
    c.expected_error === 'mesh contains no triangles, lines or points' &&
    new RegExp(`^((write ${c.to}: )?unsupported mesh feature: ${c.to} holds [^;]+; this scene has .+ and no .+|no ${c.from} source was written)$`).test(c.error || '');
  const sameRefusal = c.error && c.expected_error && (c.error === c.expected_error || c.error === `write ${c.to}: ${c.expected_error}` || namedRefusal);
  if (sameRefusal) {
    entry.verdict = 'refused';
    entry.reasons.push(`refused: ${c.error} (the expected scene cannot be written either: ${c.expected_error})`);
    process.stdout.write(`ref  ${c.fixture} ${c.from} -> ${raw.to}  ${c.error}\n`);
    continue;
  }
  if (c.error && c.expected_error) {
    entry.verdict = 'fail';
    entry.reasons.push(`conversion error: ${c.error}, not the expected scene's own: ${c.expected_error}`);
    failures++;
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
  // A dae-keep file: refused by three.js and assimp when it holds a <morph>
  // (decided per file: a fixture with no targets writes the file `bake` does),
  // and judged by xmllint either way.
  const refuse = c.variant === DAE_KEEP && hasMorph(await fs.readFile(primary, 'utf8'));
  if (c.variant === DAE_KEEP) {
    entry.validation.push(await xmllintRecord(primary));
    if (refuse) {
      daeKeepTotals.morphFiles++;
      entry.refusals = {};
    }
  }
  const expectedValidation = report.fixtures[c.fixture]?.expected?.[c.expected]?.validation || [];
  for (const v of entry.validation) {
    // assimp must refuse a file with a <morph> (dae-keep.mjs ASSIMP_KEEP_CITE).
    if (refuse && v.tool === 'assimp') {
      const k = assimpKeepVerdict(v);
      if (k.refused) {
        v.refused = k.cite;
        entry.refusals.assimp = { cite: k.cite, error: k.error };
        daeKeepTotals.assimpRefused++;
      } else {
        entry.reasons.push(k.fail);
      }
      continue;
    }
    // Our own fixtures' glTF outputs carry no validator warning either (0
    // across 161 files on 2026-11-04, and the material layers' extensions
    // must add none): a warning fails the case. A third-party original may
    // warn for reasons of its own, so its cases are held to errors alone.
    if (v.ok && v.tool === 'gltf-validator' && v.warnings > 0 && !fx.native) {
      entry.reasons.push(`gltf-validator warned on the file: ${v.messages.slice(0, 3).join(' / ')}`);
      continue;
    }
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
    // assimp 6.0.5 puts KHR_materials_specular's specularTexture in its
    // SPECULAR slot 0 and specularColorTexture in slot 1
    // (code/AssetLib/glTF2/glTF2Importer.cpp:293-294), then its own
    // validation refuses any material whose texture slots have a gap
    // (code/PostProcessing/ValidateDataStructure.cpp:514-516, "%s #%i is
    // set, but there are only %i %s textures"). The extension makes the two
    // textures independent, so a material with a specularColorTexture and
    // no specularTexture is a valid file assimp refuses. Excused only when
    // the expected GLB, written from the same scene, is refused alike.
    const specularSlot = (x) => x.tool === 'assimp' && !x.ok && /Specular #1 is set, but there are only 1 Specular textures/.test((x.messages || []).join(' '));
    if (specularSlot(v) && expectedValidation.some(specularSlot)) {
      v.skipped = 'assimp refuses a KHR_materials_specular material with a specularColorTexture and no specularTexture (glTF2Importer.cpp:293-294 fills SPECULAR slot 1 alone; ValidateDataStructure.cpp:514-516 rejects the gap); the expected GLB is refused alike';
      continue;
    }
    // A PLY of points alone (a point cloud) has no face for assimp's validation
    // (WIRE_ASSIMP_NO_FACES): a cited skip, counted apart from the comparisons.
    // Only where the expected scene has no triangle either: a PLY writer that
    // dropped the faces of a scene that has them is no point cloud, and its
    // refusal stays a failure.
    if (v.tool === 'assimp' && c.to === 'ply' && /contains no faces/.test((v.messages || []).join(' '))) {
      const held = await page.evaluate((k) => window.expectedStats?.(k), `${c.fixture}|${c.expected}`);
      if (held && held.triangles === 0) {
        v.skipped = WIRE_ASSIMP_NO_FACES;
        wireSkips.assimp++;
        continue;
      }
    }
    entry.reasons.push(`${v.tool} rejected the file${v.messages ? ': ' + v.messages.slice(0, 3).join(' / ') : ''}`);
  }
  if (fx.native && GLTF_NATIVE[fx.native.format] && GLTF_NATIVE[c.to] && LAYER_SOURCES.has(c.from)) {
    layerExtensionCheck(entry, path.join(matrixDir, fx.native.path), primary);
  }
  // What the expected picture holds, and so which uv sets are compared, and
  // by which reader.
  const info = await page.evaluate((k) => window.expectedStats(k), `${c.fixture}|${c.expected}`);
  // Where the expected scene has morph targets the dae-keep file must hold a
  // <morph>: a writer that regressed to baking would write an ordinary Collada
  // file that every judge passes.
  if (c.variant === DAE_KEEP) {
    const noMorph = keepMissingMorph(!!info?.morphed, refuse);
    if (noMorph) entry.reasons.push(noMorph);
  }
  const uvPlan = uvSetPlan(c, info);
  const lit = litPlan(c, info, litVisibility.get(`${c.fixture}|${c.expected}`));
  entry.layers = info?.layers || [];
  if (lit.unseen?.length) entry.litUnseen = lit.unseen;
  if (lit.fail) entry.reasons.push(lit.fail);
  const cl = camlightPlan(c, info);
  // The expected scene's points and lines (WIRE): three.js compares with the
  // expected picture as its loader would draw it.
  const wire = wirePlan(c, info);
  const job = {
    url: `${base}/m/${c.primary}`,
    format: c.to,
    files: c.files,
    camera: fx.camera,
    modes: [...modesFor(c.caps), ...uvPlan.three, ...lit.three, ...cl.three],
    times: c.caps.animation ? fx.times : [],
    expectedKey: `${c.fixture}|${c.expected}${wire.three ? `#${wire.three.blind}` : ''}`,
    clipName: fx.animations?.[0],
    // A USD file with several clips (an `Animation` variant set) selects the
    // first, and USDLoader lists a clip per SkelAnimation without reading the
    // variant set, so only the first one plays (page/render.js, playClip).
    ...(USD_FORMATS[c.to] && fx.animations?.length > 1 ? { playClip: fx.animations[0] } : {}),
    // A USDZ whose root layer is a crate (usdz-crate): read as the crate it holds.
    ...(crateRooted(c) ? { crateRoot: true } : {}),
    // The expected first camera, by any of its node's names (CAMLIGHT).
    ...(info?.cameras?.length ? { cameraName: nameCandidates(info.cameras[0].name) } : {}),
  };
  // A crate holding an empty array is not three.js's to read (THREE_CRATE_EMPTY_ARRAY).
  const emptyArrays = crateRooted(c) ? await threeCrateEmptyArray(primary) : null;
  const r = emptyArrays ? { skippedCrate: emptyArrays } : await page.evaluate((j) => window.renderCase(j), job);
  if (refuse) {
    // three.js ColladaLoader must fail the whole file (THREE_KEEP_CITE); what
    // it drew, if it drew, is not compared.
    const k = threeKeepVerdict(r.error);
    if (k.refused) {
      entry.refusals.three = { cite: k.cite, error: k.error };
      daeKeepTotals.threeRefused++;
    } else {
      entry.reasons.push(k.fail);
    }
  } else if (r.skippedCrate) {
    entry.results['three:crate'] = { static: { skipped: THREE_CRATE_EMPTY_ARRAY(r.skippedCrate) } };
    USD_ITEM6.threeCrateEmptyArraySkips++;
  } else if (r.error) {
    entry.reasons.push(`${c.to} loader failed: ${r.error}`);
  } else {
    entry.stats = r.stats;
    if (r.stats.texturesLoaded === false) entry.reasons.push('a texture the file references never loaded');
    for (const [mode, byT] of Object.entries(r.results)) {
      entry.results[mode] = {};
      for (const [t, res] of Object.entries(byT)) {
        const dir = path.join(outDir, 'img', c.fixture, caseId(c));
        const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, maxDelta: res.maxDelta };
        if (res.skipped) {
          item.skipped = res.skipped;
          // A skipped lit picture is kept: the reader is not its judge, but
          // what it drew is still evidence.
          if (res.image) item.image = await saveDataUrl(res.image, path.join(dir, `${mode}_${t}.png`));
        } else if (res.error) {
          item.error = res.error;
          entry.reasons.push(`${mode}@${t}: ${res.error}`);
        } else {
          item.image = await saveDataUrl(res.image, path.join(dir, `${mode}_${t}.png`));
          // A mode without a threshold would pass whatever it shows.
          if (THRESHOLD[mode] === undefined) {
            entry.reasons.push(`${mode}@${t}: no threshold for this mode`);
          } else if (res.ratio > THRESHOLD[mode]) {
            item.diff = await saveDataUrl(res.diff, path.join(dir, `${mode}_${t}_diff.png`));
            // A cited, measured lit_coat skip (Ruling A39), within its bound.
            const measured = mode === 'lit_coat' ? LIT_COAT_MEASURED[`${c.fixture}|${c.from}|${c.to}`] : null;
            if (measured && res.differ <= measured.maxDiffer) item.skipped = measured.reason;
            else entry.reasons.push(`${mode}@${t}: ${(res.ratio * 100).toFixed(2)}% of pixels differ`);
          }
        }
        entry.results[mode][t] = item;
      }
    }
  }
  for (const [mode, reason] of Object.entries(uvPlan.skips)) entry.results[mode] = { static: { skipped: reason } };
  for (const [mode, reason] of Object.entries(lit.skips)) entry.results[mode] = { static: { skipped: reason } };
  for (const [mode, reason] of Object.entries(cl.skips)) entry.results[mode] = { static: { skipped: reason } };
  // What three.js's loader for this format cannot read of the expected scene's
  // points and lines: the comparison above used the expected picture without it.
  if (wire.three) wireSkip(entry, 'three', 'wire', wire.three.skip);
  // A glTF output holds the expected scene's cameras and lights, no more and
  // no fewer, as GLTFLoader reads them (CAMLIGHT).
  if (GLTF_NATIVE[c.to] && !r.error) {
    for (const k of ['cameras', 'lights']) {
      const got = r.stats[k]?.length ?? 0;
      const want = info?.[k]?.length ?? 0;
      if (got !== want) entry.reasons.push(`the file has ${got} ${k.slice(0, -1)}(s), the expected scene ${want}`);
    }
  }
  // A set three.js judged in no frame for a reason of its own goes to SceneKit
  // too, where SceneKit reads the format (`uvAlso`): where SceneKit draws the
  // uvs apart from a pose, its picture judges uv data. Not in a USD file with
  // BlendShapes: there every uv set three.js does not judge, the plan's
  // SceneKit sets included, is the cited skip USD_BLENDSHAPE_UV. Unlike the
  // plan's own sets, nothing fails when SceneKit is missing: these were
  // unjudged before it.
  const blendShapeUsd = !!USD_FORMATS[c.to] && !!info?.morphed;
  const uvUnjudged = ['uv', 'uv1', 'uv2', 'uv3'].slice(0, Math.min(uvPlan.sets, 4)).filter(
    (mode) => !Object.values(entry.results[mode] || {}).some((x) => x && !x.skipped),
  );
  const uvPoseOnly = blendShapeUsd ? uvUnjudged : [];
  const uvAlso = blendShapeUsd ? [] : uvUnjudged.filter((mode) => !uvPlan.scenekit.includes(mode));
  const uvSceneKit = blendShapeUsd ? [] : [...uvPlan.scenekit, ...uvAlso];
  // Second opinion from SceneKit, for the formats it reads.
  if (SCENEKIT_FORMATS.has(c.to) && (await scenekitBinary())) {
    entry.scenekit = {};
    for (const mode of uvPoseOnly) entry.scenekit[mode] = { skipped: USD_BLENDSHAPE_UV };
    // What SceneKit's importer cannot read of the expected scene's points and
    // lines (WIRE): cited skips. Its other pictures draw the triangle elements
    // alone (render.swift), so they compare with the expected picture without
    // the points and lines; a `wire` picture judges the rest.
    for (const [key, reason] of Object.entries(wire.scenekit.skips)) wireSkip(entry, 'scenekit', key, reason);
    const skKey = `${c.fixture}|${c.expected}${wire.any ? '#wire' : ''}`;
    const dir = path.join(outDir, 'img', c.fixture, caseId(c));
    const times = [null, ...(c.caps.animation ? fx.times : [])];
    let clip = null;
    let notPlayed = null;
    const restSkip = USD_FORMATS[c.to] && info?.morphDefaults ? SCENEKIT_USD_MORPH_DEFAULTS : null;
    const alphaSkip = info?.translucent || info?.textureAlpha || info?.vertexAlpha ? ALPHA_ORACLE_SKIP : null;
    const albedoSkip = c.to === 'obj' && (await mtlTintsAMap(path.join(matrixDir, c.dir), c.files)) ? SCENEKIT_OBJ_KD : null;
    // SceneKit's two USD albedo defects (colour values read as Display P3,
    // primvar readers skipped) act only on colours strictly between 0 and 1
    // and on vertex colours: where the expected scene has neither, its USD
    // albedo is judged like any other (page/render.js, usdAlbedoJudgeable).
    const usdAlbedo = !!USD_FORMATS[c.to] && (await page.evaluate((k) => window.usdAlbedoJudgeable(k), `${c.fixture}|${c.expected}`));
    // And it ignores UsdUVTexture's inputs:scale, the tint a base colour puts
    // on a base-colour texture (scenekit-limits.mjs, SCENEKIT_USD_TEXSCALE).
    const usdTint = usdAlbedo && TOOLS.usdcat ? usdTextureTint((await run('usdcat', ['--flatten', primary], { timeout: 120000, maxBuffer: 256 << 20 }).catch(() => ({ stdout: '' }))).stdout) : false;
    if (c.to === 'dae' && times.length > 1) ({ clip, notPlayed } = sceneKitClip(await fs.readFile(primary, 'utf8'), fx.animations?.[0]));
    // A USD file with several clips (item 6) holds a SkelAnimation prim per
    // clip, and SceneKit plays them all at once (the note above SCENEKIT_VARIANT_SKIP),
    // so it is told to play the first clip's player alone: its own picture of the
    // clip the file selects.
    const several = USD_FORMATS[c.to] && times.length > 1 && TOOLS.usdcat && ((await usdRootText(primary)).match(/\bdef SkelAnimation\b/g) || []).length > 1;
    const playFirst = several && fx.animations?.[0] ? { playClip: fx.animations[0] } : {};
    if (several) USD_ITEM6.clips.sceneKitSelected++;
    // A dae-keep file's SceneKit uv frames (Ruling DM20, keepUvJudge): Keep
    // changes no texture coordinate, and SceneKit draws the plain Collada
    // file's (the bake's) uvs identically, so its rest picture is judged against
    // SceneKit's own picture of that file; the plain file is the same fixture
    // and source written without the morphs.
    const plainCase =
      c.variant === DAE_KEEP ? manifest.cases.find((x) => x.fixture === c.fixture && x.from === c.from && x.to === 'dae' && !x.error && x.primary) : null;
    const keepUvCounterpart = async (mode, channel) => {
      if (!plainCase) return null;
      const plainPng = path.join(dir, `scenekit_${mode}_plain.png`);
      await fs.mkdir(dir, { recursive: true });
      const draw = () => scenekitRender(path.join(matrixDir, plainCase.primary), fx.camera, 'uv', null, plainPng, null, channel);
      const ask = (query) =>
        page.evaluate(([url, key, m]) => window.compareImage(url, key, m, 'static'), [`${base}/out/${path.relative(outDir, plainPng)}${query}`, skKey, mode]);
      const r = await draw();
      if (!r || r.error) return null;
      let cmp = await ask('');
      // An empty frame where a picture is expected is rendered again once, as below.
      if (!cmp.error && cmp.gotDrawn === 0 && cmp.wantDrawn > 0) {
        const again = await draw();
        if (again && !again.error) cmp = await ask('?retry=1');
      }
      if (cmp.error || cmp.gotDrawn === 0) return null;
      return { png: plainPng, image: path.relative(outDir, plainPng), expectedRatio: cmp.ratio };
    };
    // Then the uv sets three.js's loader for this format does not build, each
    // through its own `mappingChannel` (render.swift's uv mode).
    for (const mode of ['silhouette', 'albedo', ...uvSceneKit, ...(wire.scenekit.expected ? ['wire'] : [])]) {
      const channel = mode.startsWith('uv') ? Number(mode.slice(2)) : null;
      const skMode = channel === null ? mode : 'uv';
      if (SCENEKIT_SKIP[`${c.to}:${mode}`] && !(mode === 'albedo' && usdAlbedo)) {
        entry.scenekit[mode] = { skipped: SCENEKIT_SKIP[`${c.to}:${mode}`] };
        continue;
      }
      if (mode === 'albedo' && albedoSkip) {
        entry.scenekit[mode] = { skipped: albedoSkip };
        continue;
      }
      if (mode === 'albedo' && usdTint) {
        entry.scenekit[mode] = { skipped: SCENEKIT_USD_TEXSCALE };
        continue;
      }
      entry.scenekit[mode] = {};
      const keepUv = c.variant === DAE_KEEP && skMode === 'uv' ? await keepUvCounterpart(mode, channel) : null;
      for (const t of times) {
        const key = t === null ? 'static' : String(t);
        if (t !== null && notPlayed) {
          entry.scenekit[mode][key] = { skipped: notPlayed };
          continue;
        }
        if (t === null && restSkip) {
          entry.scenekit[mode][key] = { skipped: restSkip };
          continue;
        }
        // The judge of a dae-keep file's uv frame (DM20): its plain
        // counterpart at rest, the expected picture for an animated frame
        // unless the counterpart itself fails at rest.
        const uvJudge = keepUv ? keepUvJudge({ rest: t === null, counterpart: keepUv, limit: 0.02 }) : null;
        if (uvJudge?.skip) {
          entry.scenekit[mode][key] = { skipped: uvJudge.skip, counterpart: { image: keepUv.image, expectedRatio: keepUv.expectedRatio } };
          continue;
        }
        const png = path.join(dir, `scenekit_${mode}_${key}.png`);
        await fs.mkdir(dir, { recursive: true });
        let r = await scenekitRender(primary, fx.camera, skMode, t, png, clip, channel, playFirst);
        if (!r || r.error) {
          entry.scenekit[mode][key] = { error: r ? r.error : 'renderer unavailable' };
          if (r && r.error) entry.reasons.push(`SceneKit could not render the ${c.to}: ${r.error}`);
          continue;
        }
        const compareTo = (query) =>
          mode === 'wire'
            ? page.evaluate((j) => window.compareWire(j), { url: `${base}/out/${path.relative(outDir, png)}${query}`, expectedKey: `${c.fixture}|${c.expected}`, mode: wire.scenekit.expected, t: key, blobs: wire.scenekit.expected === 'wire_cover_points' ? wire.pointVertices : 0 })
            : page.evaluate(
                ([url, expectedKey, mode, key]) => window.compareImage(url, expectedKey, mode, key),
                [`${base}/out/${path.relative(outDir, png)}${query}`, skKey, mode, key],
              );
        let cmp = await compareTo('');
        // SceneKit's offscreen renderer now and then hands back an empty frame
        // for a file it draws: on 2026-11-01 a PLY it drew in the silhouette
        // pass came back blank in the albedo pass of the same run, and drew on
        // the next. An empty frame where a picture is expected is re-rendered
        // once (fetched past the browser's cache), and only a second counts.
        let retried = false;
        if (!cmp.error && cmp.gotDrawn === 0 && cmp.wantDrawn > 0) {
          const again = await scenekitRender(primary, fx.camera, skMode, t, png, clip, channel, playFirst);
          if (again && !again.error) {
            r = again;
            cmp = await compareTo('?retry=1');
            retried = true;
          }
        }
        const item = { image: path.relative(outDir, png), ratio: cmp.ratio, stats: r.stats, ...(retried ? { retried: 'SceneKit drew an empty frame; rendered again' } : {}) };
        let limit = mode === 'wire' ? WIRE_LIMIT : mode === 'silhouette' ? 0.01 : 0.02;
        let diff = mode === 'wire' ? cmp.overlay : cmp.diff;
        // The wire picture's judge is the box and the centroid (WIRE).
        if (mode === 'wire') Object.assign(item, { apart: cmp.apart, want: cmp.want, got: cmp.got });
        // Albedo is judged on 4x4 block means (Ruling A38).
        if (mode === 'albedo' && !cmp.error) {
          const v = await scenekitAlbedoVerdict(`${base}/out/${path.relative(outDir, png)}${retried ? '?retry=1' : ''}`, skKey, key, cmp);
          Object.assign(item, { ratio: v.ratio, pixelRatio: v.pixelRatio, blocks: v.blocks, judge: v.judge });
          limit = v.limit;
          diff = v.diff;
        }
        // A dae-keep file's rest uv picture against SceneKit's own picture of
        // the plain Collada file (DM20); null when either cannot be read, and
        // the expected picture judges, as it does an animated frame.
        const vsPlain = uvJudge?.against === 'counterpart' && !cmp.error && !cmp.empty ? await keepUvAgainstPlain(keepUv.png, png) : null;
        if (cmp.error) {
          // A comparison that could not run judged nothing, so it fails the
          // case, as a render that failed does.
          item.error = cmp.error;
          entry.reasons.push(`SceneKit ${mode}@${key}: ${cmp.error}`);
        } else if (alphaSkip && (mode === 'silhouette' || mode === 'albedo')) {
          // Alpha the expected pictures do not draw (Ruling A37): kept, not judged.
          item.skipped = alphaSkip;
          if (diff) item.diff = await saveDataUrl(diff, path.join(dir, `scenekit_${mode}_${key}_diff.png`));
        } else if (cmp.empty) {
          item.skipped = WIRE_EMPTY;
        } else if (vsPlain) {
          // The ratio against the expected picture stays beside the judged
          // one as information.
          Object.assign(item, {
            expectedRatio: item.ratio,
            ratio: vsPlain.ratio,
            judge: 'plain dae',
            counterpart: { image: keepUv.image, expectedRatio: keepUv.expectedRatio },
          });
          if (item.ratio > limit) {
            entry.reasons.push(
              `SceneKit ${mode}@${key}: ${(item.ratio * 100).toFixed(2)}% differ from SceneKit's picture of the plain Collada file (against the expected picture ${(item.expectedRatio * 100).toFixed(2)}%)`,
            );
          }
        } else if (item.ratio > limit) {
          item.diff = await saveDataUrl(diff, path.join(dir, `scenekit_${mode}_${key}_diff.png`));
          entry.reasons.push(
            mode === 'wire'
              ? `SceneKit wire@${key}: ${wireApart(cmp)}`
              : `SceneKit ${mode}@${key}: ${(item.ratio * 100).toFixed(2)}% ${item.judge === 'blocks' ? 'of 4x4 blocks ' : ''}differ`,
          );
        }
        entry.scenekit[mode][key] = item;
      }
    }
  } else if (uvPlan.scenekit.length && !blendShapeUsd) {
    // As a missing usdrecord fails a USD case: these sets have no other judge.
    entry.reasons.push(`SceneKit is not available, and nothing else judges uv set(s) ${uvPlan.scenekit.map((m) => m.slice(2)).join(', ')} of this ${c.to}`);
  }
  if (USD_FORMATS[c.to] && !r.error) {
    await usdOracle(c, fx, entry, r.stats, !!info?.layers?.length, info?.translucent || info?.textureAlpha || info?.vertexAlpha ? ALPHA_ORACLE_SKIP : null, wire);
    if (!geometryJudged(entry)) entry.reasons.push(USD_NO_GEOMETRY_JUDGE);
    // Item 6: usdcat's verdict, several clips, unlit.
    await usdItem6Checks(c, fx, entry, primary, info);
  }
  // A file with a <morph> that the other readers refuse is judged by SceneKit
  // alone (Ruling DM18): its silhouette must be judged at rest and at every
  // sample time of the case, so no SceneKit, or a skipped or failed frame
  // (`notPlayed`), is a failed case. Except the clips-share-a-node skip of every
  // animated frame (Ruling DM21): the case passes on its rest frame, marked and
  // counted apart in the totals.
  if (refuse) {
    const keys = ['static', ...(c.caps.animation ? fx.times : []).map(String)];
    const judge = keepSilhouetteVerdict(entry.scenekit?.silhouette, keys);
    if (judge.fail) entry.reasons.push(judge.fail);
    else if (judge.animatedUnjudged) {
      entry.animatedUnjudged = judge.animatedUnjudged;
      daeKeepTotals.animatedUnjudged++;
    }
  }
  // CAMLIGHT: the numeric oracle on every FBX, Collada, glTF and USD output
  // (it also fails a light or camera the expected scene does not have), and
  // the other renderers' camera and light judges.
  if (ORACLE_FORMATS[c.to] && refuse) {
    // assimp is the oracle's reader of Collada, and it refuses this file
    // (ASSIMP_KEEP_CITE): the cameras and lights are judged by SceneKit alone.
    if (info?.cameras?.length || info?.lights?.length) entry.camlightOracle = { skipped: ASSIMP_KEEP_CITE };
  } else if (ORACLE_FORMATS[c.to]) {
    const oracle = await camlightOracleCase(entry, c, primary, c.caps.animation ? fx.times || [] : []);
    if (c.to === 'fbx' && (cl.camera || cl.lights)) await assimpCamlight(entry, c, fx, oracle, cl, info);
  }
  if (cl.camera || cl.lights) {
    await scenekitCamlight(entry, c, fx, primary, cl, info);
    await usdrecordCamlight(entry, c, fx, primary, cl, info);
    camlightJudged(entry, cl, info);
  }
  if (entry.reasons.length) {
    entry.verdict = 'fail';
    failures++;
  }
  const flag = entry.verdict === 'pass' ? 'ok  ' : 'FAIL';
  process.stdout.write(`${flag} ${c.fixture} ${c.from} -> ${raw.to}${entry.reasons.length ? '  ' + entry.reasons.slice(0, 2).join('; ') + (entry.reasons.length > 2 ? ` (+${entry.reasons.length - 2} more in report.json)` : '') : ''}\n`);
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
    modes: ['cover', 'albedo', 'wire_cover_points', 'wire_cover_lines'],
    presentUvSetsOnly: true,
    times,
    clipName: fx.animations?.[0],
  });
  const camLayer = usdCameraLayer(nativeCam, up);
  // Without the original's own lights, as usdOracle's passes. The reading's
  // Points and curves are judged in passes of their own (WIRE): the cover and
  // albedo passes leave those prims out and are compared with the reading
  // drawn without them (`#wire`).
  const wire = wirePlan({ to: fx.native.format }, await page.evaluate((k) => window.expectedStats?.(k), `native|${name}`));
  const off = usdLightPaths(text);
  const prims = (re) => (wire.any ? usdPrims(text).filter((p) => re.test(p.type)).map((p) => p.path) : []);
  const [pointsOff, curvesOff, meshOff] = [prims(/^Points$/), prims(USD_CURVE_PRIMS), prims(/^Mesh$/)];
  const blind = wire.any ? '#wire' : '';
  const passes = [
    { mode: 'cover', kind: 'silhouette', session: camLayer + usdOvers(usdLightsOff([...off, ...pointsOff, ...curvesOff])), args: [] },
    { mode: 'albedo', kind: 'albedo', session: camLayer + usdAlbedoOverrides(text, [...off, ...pointsOff, ...curvesOff]), args: ['--disableCameraLight'] },
  ];
  if (wire.points) passes.push({ mode: 'wire_points', kind: 'wire', expected: 'wire_cover_points', blobs: wire.pointVertices, width: WIRE_USD_WIDTH, session: camLayer + usdOvers(usdLightsOff([...off, ...meshOff, ...curvesOff])), args: [] });
  if (wire.lines) passes.push({ mode: 'wire_lines', kind: 'wire', expected: 'wire_cover_lines', width: WIRE_USD_WIDTH, session: camLayer + usdOvers(usdLightsOff([...off, ...meshOff, ...pointsOff])), args: [] });
  const dir = path.join(outDir, 'img', name, 'native');
  for (const pass of passes) {
    const frames = await usdrecordFrames(nativeFile, pass.session, times, tcps, work, pass.mode, pass.args, pass.width ? { width: pass.width } : {});
    for (const [label, file] of Object.entries(frames)) {
      const key = `usdrecord:${pass.mode}@${label}`;
      if (!file) {
        check.reasons.push(`${key}: usdrecord failed on the original`);
        continue;
      }
      const res = await page.evaluate((j) => (j.kind === 'wire' ? window.compareWire(j) : window.compareExternal(j)), {
        url: `${base}/m/${rel}/${path.basename(file)}`,
        expectedKey: `native|${name}${pass.kind === 'wire' ? '' : blind}`,
        mode: pass.expected || pass.mode,
        t: label,
        kind: pass.kind,
        width: pass.width,
        blobs: pass.blobs,
      });
      check[key] = {
        ratio: res.ratio,
        native: await saveDataUrl(res.image, path.join(dir, `usdrecord_${pass.mode}_${label}.png`)),
        diff: res.diff || res.overlay ? await saveDataUrl(res.diff || res.overlay, path.join(dir, `usdrecord_${pass.mode}_${label}_diff.png`)) : null,
        ...(pass.kind === 'wire' ? { apart: res.apart, want: res.want, got: res.got } : {}),
      };
      if (res.error) check.reasons.push(`${key}: ${res.error}`);
      else if (pass.kind === 'wire') {
        if (res.empty) check[key].skipped = WIRE_EMPTY;
        else if (res.ratio > WIRE_LIMIT) check.reasons.push(`${key}: ${wireApart(res)}`);
      } else if (res.ratio > USD_THRESHOLD[pass.mode]) check.reasons.push(`${key}: ${(res.ratio * 100).toFixed(1)}% differ from usdrecord's rendering of the original`);
    }
  }
}

// A dome light for usdrecord's lit pass: UsdLuxDomeLight in the session layer
// beside the camera, white, at USDRECORD_DOME_INTENSITY, through a constant
// white texture file. Storm lights nothing specular from a dome without a
// texture ("Dome light has no texture asset path",
// pxr/imaging/hdSt/simpleLightingShader.cpp:277 at OpenUSD v25.02 and v25.05,
// the installed Apple USD Tools 0.25.2 — :283 at v25.08, :289 at v25.11;
// measured: a metal reads black), so the constant colour is a file.
function usdDomeLight(textureFile, intensity = USDRECORD_DOME_INTENSITY) {
  return `    def DomeLight "VerifyDome"
    {
        color3f inputs:color = (1, 1, 1)
        float inputs:intensity = ${intensity}
        asset inputs:texture:file = @${textureFile}@
    }
`;
}
// An 8x4 white RGB PNG, written beside the usdrecord frames.
function whitePng() {
  const crc32 = (buf) => {
    let c;
    let crc = 0xffffffff;
    for (let n = 0; n < buf.length; n++) {
      c = (crc ^ buf[n]) & 0xff;
      for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
      crc = (crc >>> 8) ^ c;
    }
    return (crc ^ 0xffffffff) >>> 0;
  };
  const chunk = (type, data) => {
    const len = Buffer.alloc(4);
    len.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([len, body, crc]);
  };
  const w = 8;
  const h = 4;
  const raw = Buffer.alloc((w * 3 + 1) * h, 255);
  for (let y = 0; y < h; y++) raw[y * (w * 3 + 1)] = 0;
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

// `alphaSkip`: ALPHA_ORACLE_SKIP where the expected file has translucency or
// texture alpha (Ruling A37); the cover and albedo pictures are kept, unjudged.
// `wire`: wirePlan's, where the expected scene has points or lines.
async function usdOracle(c, fx, entry, stats, layered, alphaSkip = null, wire = null) {
  const primary = path.join(matrixDir, c.primary);
  const times = c.caps.animation ? fx.times : [];
  const work = path.join(matrixDir, c.dir, 'usdrecord');
  const rel = path.relative(matrixDir, work).split(path.sep).join('/');
  const imgDir = path.join(outDir, 'img', c.fixture, caseId(c));
  const text = await usdRootText(primary);
  const tcps = Number((text.match(/timeCodesPerSecond = ([0-9.eE+-]+)/) || [])[1] || 24);
  const camera = usdCameraLayer(fx.camera);
  if (!TOOLS.usdrecord) {
    entry.reasons.push('usdrecord is not installed; USD is only half verified');
    return;
  }
  // No pass here judges the file's lights: every one draws without them
  // (usdLightPaths; the lights are judged by usdrecordCamlight).
  let off = [];
  // The Points and curve prims, which the `cover` and `albedo` passes leave out
  // (the expected picture is drawn without them, `#wire`), and the Mesh prims,
  // which the wire passes leave out (WIRE).
  let pointsOff = [];
  let curvesOff = [];
  let meshOff = [];
  try {
    const flat = (await run('usdcat', ['--flatten', primary], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
    off = usdLightPaths(flat);
    if (wire?.any) {
      const of = (re) => usdPrims(flat).filter((p) => re.test(p.type)).map((p) => p.path);
      pointsOff = of(/^Points$/);
      curvesOff = of(USD_CURVE_PRIMS);
      meshOff = of(/^Mesh$/);
    }
  } catch (e) {
    entry.reasons.push(`usdcat cannot flatten the file, so its lights cannot be switched off: ${String(e.stderr || e.message).slice(0, 200)}`);
  }
  const lightsOff = usdOvers(usdLightsOff([...off, ...pointsOff, ...curvesOff]));
  const blind = wire?.any ? '#wire' : '';
  const passes = [
    { mode: 'cover', kind: 'silhouette', session: camera + lightsOff, args: [], blind, ...(alphaSkip ? { skipped: alphaSkip } : {}) },
    { mode: 'albedo', kind: 'albedo', session: camera + usdAlbedoOverrides(text, [...off, ...pointsOff, ...curvesOff]), args: ['--disableCameraLight'], blind, ...(alphaSkip ? { skipped: alphaSkip } : {}) },
  ];
  // The points, then the lines, alone, with the Mesh prims deactivated (and the
  // other kind): judged on the coverage's bounding box and centroid against
  // the expected `wire_cover_points` and `wire_cover_lines` pictures, as Storm
  // draws a point at its world-space `widths` (WIRE).
  if (wire?.usdrecord && wire.points) {
    passes.push({ mode: 'wire_points', kind: 'wire', expected: 'wire_cover_points', blobs: wire.pointVertices, width: WIRE_USD_WIDTH, session: camera + usdOvers(usdLightsOff([...off, ...meshOff, ...curvesOff])), args: [] });
  }
  if (wire?.usdrecord && wire.lines) {
    passes.push({ mode: 'wire_lines', kind: 'wire', expected: 'wire_cover_lines', width: WIRE_USD_WIDTH, session: camera + usdOvers(usdLightsOff([...off, ...meshOff, ...pointsOff])), args: [] });
  }
  // The lit pass, where the expected scene keeps a layer: compared with the
  // flat-lit expected picture (`lit_flat`, a uniform white environment, what
  // the dome is), kept but not judged (USDRECORD_LIT_NOT_A_JUDGE).
  if (layered) {
    await fs.mkdir(work, { recursive: true });
    const tex = path.join(work, 'dome_white.png');
    await fs.writeFile(tex, whitePng());
    passes.push({ mode: 'lit', expected: 'lit_flat', kind: 'albedo', session: usdCameraLayer(fx.camera, [0, 1, 0], usdDomeLight(tex)) + lightsOff, args: ['--disableCameraLight'], skipped: USDRECORD_LIT_NOT_A_JUDGE });
  }
  for (const pass of passes) {
    const frames = await usdrecordFrames(primary, pass.session, times, tcps, work, pass.mode, pass.args, pass.width ? { width: pass.width } : {});
    const key = `usdrecord:${pass.mode}`;
    entry.results[key] = {};
    for (const [label, file] of Object.entries(frames)) {
      if (!file) {
        entry.reasons.push(`${key}@${label}: usdrecord failed`);
        continue;
      }
      // The wire pass is judged on the coverage's box and centroid (WIRE); the
      // others compare with the expected picture drawn as this pass draws the file.
      const res = await page.evaluate((j) => (j.kind === 'wire' ? window.compareWire(j) : window.compareExternal(j)), {
        url: `${base}/m/${rel}/${path.basename(file)}`,
        expectedKey: `${c.fixture}|${c.expected}${pass.blind ?? ''}`,
        mode: pass.expected || pass.mode,
        t: label,
        kind: pass.kind,
        width: pass.width,
        blobs: pass.blobs,
      });
      const item = { ratio: res.ratio, differ: res.differ, covered: res.covered, ...(pass.kind === 'wire' ? { apart: res.apart, want: res.want, got: res.got } : {}) };
      if (res.error) {
        item.error = res.error;
        entry.reasons.push(`${key}@${label}: ${res.error}`);
      } else if (pass.skipped) {
        item.image = await saveDataUrl(res.image, path.join(imgDir, `${key.replace(':', '_')}_${label}.png`));
        item.diff = await saveDataUrl(res.diff, path.join(imgDir, `${key.replace(':', '_')}_${label}_diff.png`));
        item.skipped = pass.skipped;
      } else if (pass.kind === 'wire') {
        item.image = await saveDataUrl(res.image, path.join(imgDir, `${key.replace(':', '_')}_${label}.png`));
        if (res.empty) {
          item.skipped = WIRE_EMPTY;
        } else if (res.ratio > WIRE_LIMIT) {
          item.diff = await saveDataUrl(res.overlay, path.join(imgDir, `${key.replace(':', '_')}_${label}_diff.png`));
          entry.reasons.push(`${key}@${label}: ${wireApart(res)}`);
        }
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
  // A file whose root layer is a crate is read by three.js as it is (the main
  // loop's geom pass), so a copy of it would judge the same bytes twice.
  const crateRoot = crateRooted(c);
  if (crateRoot && info && info.skinned && times.length) {
    entry.results['crate:geom'] = { static: { skipped: 'the root layer is already a crate, which three.js reads correctly (page/render.js usdLimitation, USD_TEXT_ROOT: only a USDA text root is misread), so the geom pass of the file itself is this check and a usdcat copy would judge the same layer twice' } };
  }
  const copyEmpty = !crateRoot && TOOLS.usdcat && info && info.skinned && times.length ? await threeCrateEmptyArray(primary) : null;
  if (copyEmpty) {
    entry.results['crate:geom'] = { static: { skipped: THREE_CRATE_EMPTY_ARRAY(copyEmpty) } };
    USD_ITEM6.threeCrateEmptyArraySkips++;
  } else if (!crateRoot && TOOLS.usdcat && info && info.skinned && times.length) {
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
        ...(fx.animations?.length > 1 ? { playClip: fx.animations[0] } : {}),
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

// ─── item 6: the checks (see USD_ITEM6 above) ────────────────────────────────

// `usdcat file`: exit code, stdout and stderr, whether it succeeds or not.
async function usdcatRun(file) {
  try {
    const { stdout, stderr } = await run('usdcat', [file], { timeout: 120000, maxBuffer: 256 << 20 });
    return { code: 0, stdout, stderr };
  } catch (e) {
    return { code: typeof e.code === 'number' ? e.code : 1, stdout: e.stdout ?? '', stderr: e.stderr ?? String(e.message) };
  }
}

async function usdItem6Checks(c, fx, entry, primary, info) {
  USD_ITEM6.active = true;
  if (!TOOLS.usdcat) return;
  const cat = await usdcatRun(primary);
  const verdict = usdcatVerdict(cat);
  USD_ITEM6.usdcat.files++;
  if (verdict.ok) USD_ITEM6.usdcat.clean++;
  entry.usdcat = { ok: verdict.ok, lines: cat.stdout.split('\n').length - 1 };
  for (const r of verdict.reasons) entry.reasons.push(`usdcat: ${r}`);
  if (!verdict.ok) return;
  if (TOOLS.usdrecord) await usdClipChecks(c, fx, entry, primary, cat.stdout);
  if (TOOLS.usdrecord && c.fixture === 'unlit' && (c.from === 'scene' || c.from === 'glb')) await usdUnlitChecks(c, fx, entry, primary, info);
}

// The Mesh prims' unique path segments for each group of a flattened stage:
// the names a reader that does not share our prim paths can be told to draw
// (a segment both groups sit under, the root, names neither).
function uniqueSegments(meshes) {
  const segs = (paths) => new Set(paths.flatMap((p) => p.split('/').filter(Boolean)));
  const [u, l] = [segs(meshes.unlit), segs(meshes.lit)];
  return { unlit: [...u].filter((s) => !l.has(s)), lit: [...l].filter((s) => !u.has(s)) };
}

// A file with several clips: each variant drawn by usdrecord against that clip's
// expected picture, usdchecker's report, three.js's list, SceneKit's cited skip.
async function usdClipChecks(c, fx, entry, primary, rootText) {
  const found = animationVariants(rootText);
  const expectedRel = manifest.fixtures[c.fixture].expected[c.expected];
  if (found.names.length < 2) {
    // Right only when the expected scene has fewer than two clips.
    if (!expectedRel) return;
    let clipNames;
    try {
      const json = readGltfJson(await fs.readFile(path.join(matrixDir, expectedRel)));
      clipNames = (json.animations || []).map((a, i) => a.name ?? `animation_${i}`);
    } catch (e) {
      entry.reasons.push(`clips: the expected scene's clips cannot be read: ${e.message}`);
      return;
    }
    const why = missingVariantsReason(found, clipNames);
    if (why) entry.reasons.push(`clips: ${why}`);
    return;
  }
  const T = USD_ITEM6.clips;
  T.files++;
  const names = found.names;
  const rec = (entry.clips = { root: found.root, selected: found.selected, variants: {} });
  let ranges;
  try {
    ranges = clipRanges(readGltfJson(await fs.readFile(path.join(matrixDir, expectedRel))));
  } catch (e) {
    entry.reasons.push(`clips: the expected scene's clips cannot be read: ${e.message}`);
    return;
  }
  // The variants' names must be the glTF's clip names (only `clips()` has
  // several today). usdcat lists a variant set's variants in name order, whatever
  // order they were authored in, so the sets are compared; the authored selection
  // is the first clip's (usd-variants.mjs, clipVariantsVerdict).
  const agree = clipVariantsVerdict(found, ranges.map((r) => r.name));
  for (const r of agree.reasons) entry.reasons.push(`clips: ${r}`);
  if (names.some((n) => !ranges.some((r) => r.name === n))) return;
  // usdchecker validates every variant of the default prim and says so.
  const checker = entry.validation.find((v) => v.tool.startsWith('usdchecker'));
  if (checker) {
    const v = usdcheckerVariantVerdict(checker.variants ?? [], names);
    rec.usdchecker = { tool: checker.tool, reported: v.reported, ok: v.ok };
    T.usdcheckerVariants += v.reported.length;
    for (const r of v.reasons) entry.reasons.push(`${checker.tool}: ${r}`);
  }
  // three.js lists a clip per SkelAnimation and the first clip's rigid motion as
  // TransformAnimation. Judged on a crate (a USDA is read at 30 time codes per
  // second, THREE_USDA_FPS): the file itself where it is one, else its usdcat copy.
  const work = path.join(matrixDir, c.dir, 'usdrecord');
  await fs.mkdir(work, { recursive: true });
  const rel = path.relative(matrixDir, work).split(path.sep).join('/');
  const crateRoot = crateRooted(c);
  const want = { clips: ranges, rigidEnd: ranges[0].rigidEnd };
  rec.three = {};
  try {
    let crate = { url: `${base}/m/${c.primary}`, format: c.to };
    if (!crateRoot) {
      await run('usdcat', [primary, '-o', path.join(work, 'clips.usdc')], { timeout: 120000 });
      crate = { url: `${base}/m/${rel}/clips.usdc`, format: 'usdc' };
    }
    const listed = await page.evaluate((j) => window.listClips(j), { ...crate, files: [] });
    const verdict = threeClipsVerdict(listed, want, { crate: true });
    rec.three.crate = { listed: listed.map((x) => `${x.name}:${x.duration.toFixed(3)}s`), ok: verdict.ok };
    for (const r of verdict.reasons) entry.reasons.push(`three.js clips (${crateRoot ? 'the crate' : 'its usdcat crate copy'}): ${r}`);
    if (!crateRoot) {
      const text = await page.evaluate((j) => window.listClips(j), { url: `${base}/m/${c.primary}`, format: c.to, files: [] });
      const tv = threeClipsVerdict(text, want, { crate: false });
      rec.three.text = { listed: text.map((x) => `${x.name}:${x.duration.toFixed(3)}s`), ok: tv.ok, durations: tv.durations };
      for (const r of tv.reasons) entry.reasons.push(`three.js clips (the file as it is): ${r}`);
    }
    T.threeListed++;
  } catch (e) {
    entry.reasons.push(`three.js clips: ${String(e.message).slice(0, 200)}`);
  }
  // usdrecord: each variant selected through a session layer, at rest and at the
  // clip's sample times, against that clip's expected picture (the clip's
  // skeleton channels, and the first clip's rigid ones: what a USD file plays
  // under every variant, page/render.js alsoRigidOf).
  const flat = (await run('usdcat', ['--flatten', primary], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  const tcps = Number((rootText.match(/timeCodesPerSecond = ([0-9.eE+-]+)/) || [])[1] || 24);
  const imgDir = path.join(outDir, 'img', c.fixture, caseId(c));
  const camera = usdCameraLayer(fx.camera) + usdOvers(usdLightsOff(usdLightPaths(flat)));
  const first = { expected: null, drawn: null, labels: [] };
  for (const [i, range] of ranges.entries()) {
    const times = clipSampleTimes(range.start, range.end);
    const key = `${c.fixture}|${c.expected}#clip:${range.name}`;
    const v = (rec.variants[range.name] = { times, usdrecord: {}, scenekit: i === 0 ? 'judged in the SceneKit passes of this case' : { skipped: SCENEKIT_VARIANT_SKIP } });
    if (i > 0) {
      T.sceneKitSkipped++;
    }
    const drawn = await page.evaluate((j) => window.renderExpected(j), {
      key,
      url: `${base}/m/${expectedRel}`,
      format: 'glb',
      camera: fx.camera,
      modes: ['cover'],
      times,
      clipName: range.name,
      ...(i > 0 ? { alsoRigidOf: ranges[0].name } : {}),
    });
    const expectedPngs = {};
    for (const [label, url] of Object.entries(drawn.images.cover)) {
      const saved = await saveDataUrl(url, path.join(imgDir, `clip_${range.name}_expected_${label}.png`));
      expectedPngs[label] = await readPng(path.join(outDir, saved));
    }
    const frames = await usdrecordFrames(primary, camera + variantOver(found.root, range.name), times, tcps, work, `clip_${range.name}`, []);
    const pngs = {};
    for (const [label, file] of Object.entries(frames)) {
      if (!file) {
        v.usdrecord[label] = { error: 'usdrecord failed' };
        entry.reasons.push(`clip ${range.name}@${label}: usdrecord failed`);
        continue;
      }
      pngs[label] = await readPng(file);
      const res = await page.evaluate((j) => window.compareExternal(j), { url: `${base}/m/${rel}/${path.basename(file)}`, expectedKey: key, mode: 'cover', t: label, kind: 'silhouette' });
      const item = { ratio: res.ratio, differ: res.differ, covered: res.covered };
      T.usdrecordFrames++;
      if (res.error) {
        item.error = res.error;
        entry.reasons.push(`clip ${range.name}@${label}: ${res.error}`);
      } else {
        item.image = await saveDataUrl(res.image, path.join(imgDir, `clip_${range.name}_usdrecord_${label}.png`));
        if (res.ratio > USD_THRESHOLD.cover) {
          item.diff = await saveDataUrl(res.diff, path.join(imgDir, `clip_${range.name}_usdrecord_${label}_diff.png`));
          entry.reasons.push(`clip ${range.name}@${label}: ${(res.ratio * 100).toFixed(2)}% of pixels differ from the clip's expected picture`);
        }
      }
      v.usdrecord[label] = item;
    }
    // A check that cannot tell one variant from another proves nothing: the
    // expected pictures of this clip must differ from the first clip's at some
    // time, and so must usdrecord's, or the selection did nothing.
    // The clips' sample times are the same fractions of their own ranges, so the
    // j-th time of one clip is compared with the j-th of the first (at rest, the
    // pictures of every clip are one).
    const labels = ['static', ...times.map(String)];
    if (i === 0) {
      first.expected = expectedPngs;
      first.drawn = pngs;
      first.labels = labels;
    } else {
      const apart = (a, b) => labels.filter((l, j) => a[l] && b[first.labels[j]] && a[l].rgba.length === b[first.labels[j]].rgba.length && pixelsDiffer(a[l].rgba, b[first.labels[j]].rgba, 0).differ > 0);
      v.apartFromFirst = { expected: apart(expectedPngs, first.expected), usdrecord: apart(pngs, first.drawn) };
      if (!v.apartFromFirst.expected.length) entry.reasons.push(`clip ${range.name}: its expected pictures equal the first clip's at every time, so no picture tells the two apart`);
      else if (!v.apartFromFirst.usdrecord.length) entry.reasons.push(`clip ${range.name}: usdrecord drew it as the first clip at every time, so the variant selection did nothing`);
    }
    T.variants++;
  }
}

// The two-light check on one USD case of the `unlit` fixture.
async function usdUnlitChecks(c, fx, entry, primary, info) {
  const T = USD_ITEM6.unlit;
  const rec = (entry.unlit = {});
  const groups = groupNames(info?.nodeChains ?? []);
  rec.groups = groups;
  if (!groups.unlit.length || !groups.lit.length) {
    entry.reasons.push(`unlit: the expected scene has ${groups.unlit.length} name(s) only on unlit meshes and ${groups.lit.length} only on lit ones; the check needs both groups`);
    return;
  }
  T.cases++;
  const flat = (await run('usdcat', ['--flatten', primary], { timeout: 120000, maxBuffer: 256 << 20 })).stdout;
  const prims = usdPrims(flat);
  const meshes = { unlit: meshesOf(prims, groups.unlit), lit: meshesOf(prims, groups.lit) };
  rec.meshes = meshes;
  if (!meshes.unlit.length || !meshes.lit.length) {
    entry.reasons.push(`unlit: the stage has ${meshes.unlit.length} Mesh prim(s) under the unlit names (${groups.unlit.join(', ')}) and ${meshes.lit.length} under the lit ones (${groups.lit.join(', ')})`);
    return;
  }
  const only = uniqueSegments(meshes);
  rec.only = only;
  const expectedPng = report.fixtures[c.fixture]?.expected?.[c.expected]?.images?.albedo?.static;
  const want = expectedPng ? await readPng(path.join(outDir, expectedPng)) : null;
  if (!want) {
    entry.reasons.push('unlit: the expected albedo picture cannot be read');
    return;
  }
  const imgDir = path.join(outDir, 'img', c.fixture, caseId(c));
  await fs.mkdir(imgDir, { recursive: true });
  const judge = (name, pics, baseSkip) => {
    const [u1, u2, l1, l2] = [pics.unlit[0], pics.unlit[1], pics.lit[0], pics.lit[1]];
    if (![u1, u2, l1, l2].every((p) => p && p.width === want.width && p.height === want.height)) {
      entry.reasons.push(`unlit ${name}: a picture is missing or not the expected picture's size`);
      return;
    }
    const masks = pureMasks(u1.rgba, l1.rgba);
    const invariance = lightInvariance(u1.rgba, u2.rgba, masks.unlit, UNLIT_LIMITS.tolerance);
    const response = lightResponse(l1.rgba, l2.rgba, masks.lit, UNLIT_LIMITS.responseTolerance);
    const baseColour = baseColourMatch(u2.rgba, want.rgba, masks.unlit, UNLIT_LIMITS.albedoTolerance, want.width, want.height);
    const verdict = unlitVerdict({ invariance, baseColour, response }, { judgeBase: !baseSkip });
    rec[name] = { ok: verdict.ok, invariance, response, baseColour: { ratio: baseColour.ratio, differ: baseColour.differ, covered: baseColour.covered, ...(baseSkip ? { recordedOnly: baseSkip } : {}) }, levels: UNLIT_LEVELS[name] };
    if (verdict.ok) T.judged[name]++;
    else for (const r of verdict.reasons) entry.reasons.push(`unlit ${name}: ${r}`);
    if (baseSkip && name !== 'usdrecord') T.baseRecordedOnly[name]++;
  };
  // usdrecord: a distant light at each level (judged), the other group's meshes
  // deactivated; then the unlit group under a dome light at each level, whose
  // residual is recorded (STORM_DOME_RESIDUAL).
  const work = path.join(matrixDir, c.dir, 'usdrecord');
  await fs.mkdir(work, { recursive: true });
  const tex = path.join(work, 'dome_white.png');
  await fs.writeFile(tex, whitePng());
  const tcps = Number((flat.match(/timeCodesPerSecond = ([0-9.eE+-]+)/) || [])[1] || 24);
  const lightsOff = usdLightsOff(usdLightPaths(flat));
  const sun = (i) => `    def DistantLight "VerifySun"
    {
        color3f inputs:color = (1, 1, 1)
        float inputs:intensity = ${i}
        bool inputs:normalize = 1
        float inputs:angle = 0
        float3 xformOp:rotateXYZ = (-35, -30, 0)
        uniform token[] xformOpOrder = ["xformOp:rotateXYZ"]
    }
`;
  const pics = { usdrecord: { unlit: [], lit: [] }, scenekit: { unlit: [], lit: [] }, three: { unlit: [], lit: [] } };
  for (const g of ['unlit', 'lit']) {
    const other = g === 'unlit' ? 'lit' : 'unlit';
    for (const [k, L] of UNLIT_LEVELS.usdrecord.entries()) {
      const session = usdCameraLayer(fx.camera, [0, 1, 0], sun(L.sun)) + usdOvers([...lightsOff, ...meshes[other].map((p) => ({ path: p, meta: ['active = false'] }))]);
      const frames = await usdrecordFrames(primary, session, [], tcps, work, `unlit_${g}_${k}`, [USDRECORD_NO_CAMERA_LIGHT]);
      if (frames.static) {
        await fs.copyFile(frames.static, path.join(imgDir, `unlit_usdrecord_${g}_${k}.png`));
        pics.usdrecord[g][k] = await readPng(frames.static);
      }
    }
  }
  judge('usdrecord', pics.usdrecord, null);
  const dome = [];
  for (const [k, intensity] of UNLIT_LEVELS.usdrecordDome.entries()) {
    const session = usdCameraLayer(fx.camera, [0, 1, 0], usdDomeLight(tex, intensity)) + usdOvers([...lightsOff, ...meshes.lit.map((p) => ({ path: p, meta: ['active = false'] }))]);
    const frames = await usdrecordFrames(primary, session, [], tcps, work, `unlit_dome_${k}`, [USDRECORD_NO_CAMERA_LIGHT]);
    if (frames.static) {
      await fs.copyFile(frames.static, path.join(imgDir, `unlit_usdrecord_dome_${k}.png`));
      dome[k] = await readPng(frames.static);
    }
  }
  if (dome[0] && dome[1] && pics.usdrecord.unlit[0] && pics.usdrecord.lit[0] && dome[0].rgba.length === dome[1].rgba.length) {
    const masks = pureMasks(pics.usdrecord.unlit[0].rgba, pics.usdrecord.lit[0].rgba);
    const r = lightInvariance(dome[0].rgba, dome[1].rgba, masks.unlit, UNLIT_LIMITS.tolerance);
    // The share of the unlit pixels past 3/255 (about 1 % of the range) too.
    const past3 = lightInvariance(dome[0].rgba, dome[1].rgba, masks.unlit, 3).ratio;
    rec.usdrecordDomeResidual = { levels: UNLIT_LEVELS.usdrecordDome, notJudged: STORM_DOME_RESIDUAL, ...r, ratioPast3: past3 };
    // A measurement, not a judge (Ruling U22): the largest delta, the largest share
    // of the unlit pixels past the 1/255 tolerance and past 3/255, and the largest
    // mean over those pixels, over the cases, with the source line of the cause.
    T.domeResidual = {
      maxDelta: Math.max(T.domeResidual?.maxDelta ?? 0, r.maxDelta),
      worstRatio: Math.max(T.domeResidual?.worstRatio ?? 0, r.ratio),
      worstRatioPast3: Math.max(T.domeResidual?.worstRatioPast3 ?? 0, past3),
      worstMeanDelta: Math.max(T.domeResidual?.worstMeanDelta ?? 0, r.meanDelta),
      cause: 'Storm adds prefilter * (F * brdf.x + brdf.y) for a dome light (/usr/lib/usd/usd/usdShaders/resources/shaders/previewSurface.glslfx:327, Apple USD Tools 0.25.2); brdf.y survives a black specular colour',
      judged: false,
    };
  }
  // SceneKit: the importer's own materials under an ambient and a directional
  // light (render.swift, lit).
  if (SCENEKIT_FORMATS.has(c.to) && (await scenekitBinary())) {
    for (const g of ['unlit', 'lit']) {
      for (const [k, lumens] of UNLIT_LEVELS.scenekit.entries()) {
        const png = path.join(imgDir, `unlit_scenekit_${g}_${k}.png`);
        const r = await scenekitRender(primary, fx.camera, 'lit', null, png, null, null, { only: only[g], lightIntensity: lumens });
        if (!r || r.error) entry.reasons.push(`unlit scenekit ${g}@${k}: ${r ? r.error : 'renderer unavailable'}`);
        else pics.scenekit[g][k] = await readPng(png);
      }
    }
    const judgeable = await page.evaluate((k) => window.usdAlbedoJudgeable(k), `${c.fixture}|${c.expected}`);
    const tinted = judgeable ? usdTextureTint(flat) : false;
    judge('scenekit', pics.scenekit, judgeable && !tinted ? null : judgeable ? SCENEKIT_USD_TEXSCALE : SCENEKIT_USD_ALBEDO);
  } else {
    rec.scenekit = { skipped: 'SceneKit is not available on this machine' };
  }
  // three.js: the loader's own materials under an ambient and a directional
  // light, no environment (page/render.js, renderLightLevels).
  const materials = {};
  for (const g of ['unlit', 'lit']) {
    const r = await page.evaluate((j) => window.renderLightLevels(j), { url: `${base}/m/${c.primary}`, format: c.to, files: c.files, camera: fx.camera, only: only[g], levels: UNLIT_LEVELS.three });
    materials[g] = r.materials;
    for (const [k, f] of r.frames.entries()) {
      const saved = await saveDataUrl(f.image, path.join(imgDir, `unlit_three_${g}_${k}.png`));
      pics.three[g][k] = await readPng(path.join(outDir, saved));
    }
  }
  const threeJudgeable = await page.evaluate((k) => window.usdAlbedoJudgeable(k), `${c.fixture}|${c.expected}`);
  judge('three', pics.three, threeJudgeable ? null : THREE_USD_BASE_SKIP);
  if (rec.three) rec.three.materials = materials;
}

// A usdc case against the usda case of the same source: usdcat prints the same
// text, and usdrecord draws every picture of the two alike.
async function usdcPairChecks() {
  const T = USD_ITEM6.usdcVsUsda;
  for (const e of report.cases) {
    if (e.to !== 'usdc' || e.verdict === 'refused' || e.error || !e.primary) continue;
    const sib = report.cases.find((x) => x.fixture === e.fixture && x.from === e.from && x.to === 'usda' && x.verdict !== 'refused' && !x.error && x.primary);
    if (!sib) continue;
    T.pairs++;
    const [a, b] = [await usdcatRun(path.join(matrixDir, sib.primary)), await usdcatRun(path.join(matrixDir, e.primary))];
    const diff = textDiff(a.stdout, b.stdout);
    const rec = (e.usdcVsUsda = { text: diff.equal ? { equal: true, lines: diff.lines } : diff, pictures: { compared: 0, identical: 0, maxDelta: 0 } });
    if (diff.equal) {
      T.textEqual++;
      T.textLines += diff.lines;
    } else {
      e.reasons.push(`usdcat prints the USDC differently from the USDA at line ${diff.line}: ${JSON.stringify(diff.a)} against ${JSON.stringify(diff.b)}`);
    }
    const [da, db] = [path.join(matrixDir, sib.dir, 'usdrecord'), path.join(matrixDir, e.dir, 'usdrecord')];
    const pngs = (await fs.readdir(da).catch(() => [])).filter((f) => f.endsWith('.png') && !f.startsWith('dome_') && fss.existsSync(path.join(db, f)));
    for (const f of pngs) {
      const [pa, pb] = [await readPng(path.join(da, f)), await readPng(path.join(db, f))];
      if (!pa || !pb || pa.rgba.length !== pb.rgba.length) {
        e.reasons.push(`usdrecord pictures ${f}: the USDA's and the USDC's cannot be compared`);
        continue;
      }
      const d = pixelsDiffer(pa.rgba, pb.rgba, 0);
      rec.pictures.compared++;
      rec.pictures.maxDelta = Math.max(rec.pictures.maxDelta, d.maxDelta);
      T.pictures++;
      T.maxDelta = Math.max(T.maxDelta, d.maxDelta);
      if (d.differ === 0) {
        rec.pictures.identical++;
        T.picturesIdentical++;
      } else {
        e.reasons.push(`usdrecord draws ${f} differently for the USDA and the USDC: ${d.differ} of ${d.pixels} pixels differ (max delta ${d.maxDelta}/255)`);
      }
    }
    // A pair with no picture compared proves nothing about the pictures.
    if (rec.pictures.compared === 0) e.reasons.push('usdrecord pictures: none of the USDA\'s and the USDC\'s could be compared');
    if (e.reasons.length && e.verdict === 'pass') {
      e.verdict = 'fail';
      failures++;
    }
  }
}
await usdcPairChecks();

await browser.close();
server.close();
report.consoleErrors = consoleErrors;
const importErrors = manifest.import_errors || [];
for (const e of importErrors) {
  process.stdout.write(`FAIL import ${e.file}: ${e.error}\n`);
  failures++;
}
report.importErrors = importErrors;
report.summary = {
  cases: report.cases.length,
  passed: report.cases.filter((c) => c.verdict === 'pass').length,
  refused: report.cases.filter((c) => c.verdict === 'refused').length,
  failed: failures,
  nativeFailed,
  // Native checks skipped with a citation (ASSET_LIMITS nativeCheck): a judge
  // measured not to decode the file, not a pass.
  nativeSkipped,
  // Point and line comparisons skipped with a citation (WIRE_* above): a
  // loader measured not to build the primitive, not a pass. Judged elsewhere
  // (usdrecord, the unit tests) or by a picture without them.
  wireSkipped: wireSkips.three + wireSkips.scenekit,
  wireSkips: { three: wireSkips.three, scenekit: wireSkips.scenekit },
  // assimp validations skipped with a citation (a PLY with no faces): not a
  // comparison, so counted apart from the point and line ones.
  validationSkipped: wireSkips.assimp,
  importsRefused: importErrors.length,
  // Which independent checks this run had: a missing one is a weaker run,
  // not a passing one, and says so here.
  tools: reportedTools({ ...TOOLS, scenekit: !!SCENEKIT }, { daeKeepCases: daeKeepTotals.cases, xmllint: daeKeepTotals.cases ? await haveXmllint() : false }),
  ...(layerChecks.cases ? { layerExtensions: { ...layerChecks } } : {}),
  // The dae-keep variant (item 8): its cases, the files that hold a <morph>,
  // how many of those three.js and assimp refused (each must), and xmllint's
  // verdicts on the files against the pinned COLLADA 1.4.1 schema.
  ...(daeKeepTotals.cases ? { daeKeep: daeKeepTotals } : {}),
  // Item 6: usdcat's verdict on every USD file, the USDC against the USDA
  // (usdcat text, usdrecord pictures), the files with several clips (variants
  // drawn, usdchecker's reports, three.js's lists, SceneKit's cited skips), and
  // the two-light unlit check per renderer.
  ...(USD_ITEM6.active ? { usd: (({ active, ...rest }) => rest)(USD_ITEM6) } : {}),
  // The numeric oracle (CAMLIGHT): outputs checked and failed (by format:
  // fbx, dae, glb and gltf through assimp, usdz and usda cameras through
  // their text), their checks passed, failed and skipped with a citation, and
  // its calibration on the expected files.
  ...(camlightTotals.files || camlightTotals.expectedFiles ? { camlightOracle: { ...camlightTotals, tolerance: ORACLE_TOLERANCE } } : {}),
};
await fs.writeFile(path.join(outDir, 'report.json'), JSON.stringify(report, null, 1));
await fs.writeFile(path.join(outDir, 'report.html'), renderHtml(report));
const refusedNote = report.summary.refused ? `, ${report.summary.refused} refused as the expected scene is` : '';
const missing = Object.entries(report.summary.tools).filter(([, ok]) => !ok).map(([t]) => t);
if (missing.length) console.log(`\nnot installed, so not checked: ${missing.join(', ')}`);
if (layerChecks.cases) {
  console.log(`\nlayer extensions (Ruling A20): ${layerChecks.cases - layerChecks.failed} of ${layerChecks.cases} glTF outputs of glTF originals keep the original's, field by field; ${layerChecks.f32Rounded} value pair(s) equal as f32 only, listed per case in report.json${layerChecks.ktx2Pixels ? `; ${layerChecks.ktx2Pixels} layer texture(s) a KTX2 image in the original and a PNG here, equal pixel for pixel (a Binomial RGBA32 transcode against the PNG)` : ''}`);
}
if (daeKeepTotals.cases) console.log(`\n${daeKeepSummary(daeKeepTotals)}`);
if (USD_ITEM6.active) {
  const u = USD_ITEM6;
  console.log(
    `\nusd (item 6): usdcat clean on ${u.usdcat.clean} of ${u.usdcat.files} files; usdc vs usda: ${u.usdcVsUsda.textEqual} of ${u.usdcVsUsda.pairs} sources print the same text, ${u.usdcVsUsda.picturesIdentical} of ${u.usdcVsUsda.pictures} usdrecord pictures identical (max delta ${u.usdcVsUsda.maxDelta}); clips: ${u.clips.files} files, ${u.clips.variants} variants drawn (${u.clips.usdrecordFrames} usdrecord frames), usdchecker reported ${u.clips.usdcheckerVariants} variants, three.js listed ${u.clips.threeListed} files, ${u.clips.sceneKitSkipped} SceneKit variants skipped with a citation; unlit: ${u.unlit.cases} cases, judged by usdrecord ${u.unlit.judged.usdrecord}, SceneKit ${u.unlit.judged.scenekit}, three.js ${u.unlit.judged.three}`,
  );
}
if (camlightTotals.files || camlightTotals.expectedFiles) {
  const t = camlightTotals;
  console.log(`\ncamlight oracle: ${t.files - t.failed} of ${t.files} outputs agree with their expected scene within ${ORACLE_TOLERANCE} (${Object.entries(t.byFormat).map(([f, n]) => `${n} ${f}`).join(', ')}; ${t.passed} checks pass, ${t.failedChecks} fail, ${t.skipped} skipped with a citation); calibration: ${t.expectedFiles - t.expectedFailed} of ${t.expectedFiles} expected files agree with themselves`);
}
const otherFailures = [nativeFailed && `${nativeFailed} native file(s) differ`, importErrors.length && `${importErrors.length} import(s) refused`, camlightTotals.expectedFailed && `${camlightTotals.expectedFailed} expected file(s) the camlight oracle disagrees with`].filter(Boolean);
const nativeSkipNote = nativeSkipped ? `; ${nativeSkipped} native check(s) skipped with a citation` : '';
const wireSkipNote = report.summary.wireSkipped ? `; ${report.summary.wireSkipped} point/line comparison(s) skipped with a citation (${wireSkips.three} three.js, ${wireSkips.scenekit} SceneKit)` : '';
const validationSkipNote = wireSkips.assimp ? `; ${wireSkips.assimp} assimp validation(s) skipped with a citation (a PLY with no faces)` : '';
console.log(`\n${report.summary.passed}/${report.summary.cases} cases pass${refusedNote}${otherFailures.length ? '; ' + otherFailures.join(', ') : ''}${nativeSkipNote}${wireSkipNote}${validationSkipNote} -> ${path.join(outDir, 'report.html')}`);
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
<p>${rep.summary.passed}/${rep.summary.cases} cases pass${rep.summary.refused ? `, ${rep.summary.refused} refused as the expected scene is` : ''}${rep.summary.nativeFailed ? `; <b>${rep.summary.nativeFailed} native file(s) differ</b>` : ''}${rep.summary.importsRefused ? `; <b>${rep.summary.importsRefused} import(s) refused</b>: ${(rep.importErrors || []).map((e) => esc(`${e.file}: ${e.error}`)).join('; ')}` : ''}. Rows: source container (scene = written directly from the fixture; single = the same, as one self-contained file). Columns: target container. Each cell is the target file rendered by three.js's own loader for that format.</p>`);
  const formats = [...new Set(rep.cases.map((c) => c.to))];
  // `dae-keep` is a target only: no file is read back from it.
  const froms = ['scene', 'single', ...formats.filter((f) => f !== DAE_KEEP)];
  for (const [name, fx] of Object.entries(rep.fixtures)) {
    // The set selector's modes, for the fixtures with more than one uv set.
    const perSet = ['uv1', 'uv2', 'uv3'].filter((m) => rep.cases.some((c) => c.fixture === name && c.results?.[m]));
    for (const mode of ['albedo', 'geom', 'normals', 'uv', ...perSet, 'lit', 'lit_coat', 'usdrecord:albedo', 'usdrecord:cover', 'usdrecord:wire_points', 'usdrecord:wire_lines', 'usdrecord:lit', 'crate:geom', 'camera', 'assimp:camera', 'usdrecord:camera', 'lit_lights', 'assimp:lit_lights']) {
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
          // The camera pictures keep their aspect.
          const wide = /camera/.test(mode) ? ' style="width:192px"' : '';
          parts.push(`<td class="${cls}"><img src="${esc(res.image)}"${wide}><small>${(res.ratio * 100).toFixed(2)}% differ</small>${res.diff ? `<img src="${esc(res.diff)}"${wide}>` : ''}</td>`);
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
