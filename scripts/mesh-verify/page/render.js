// Renders one model file with three.js's own loader for its format and
// returns pictures of it, so a converted file is judged by an independent
// reader rather than by the converter's own.
//
// Every picture is UNLIT. Loaders build different material classes for the
// same data (GLTFLoader a MeshStandardMaterial, OBJ/FBX/Collada a Phong, the
// 3MF loader its own), so lit renders would differ even when the data is
// identical. Instead each mode isolates one kind of data:
//
//   geom     face orientation from screen-space derivatives (flat shading).
//            Depends only on positions, transforms and winding: back faces
//            are culled, so a reversed winding is a hole.
//   normals  the file's vertex normals, view space, as colour.
//   albedo   base colour x texture x vertex colour, exactly as the loader
//            decoded them (including its colour-space conversion).
//   uv       a gradient texture addressed by UV set 0, so the colour of each
//            pixel IS its texture coordinate.
//   uv1–uv3  the same gradient through UV sets 1–3 (the texture's `channel`),
//            for files that carry more than one set: the set selector.
//
// Three modes are lit, because the material layers (clearcoat, sheen,
// transmission, volume, ior, specular, iridescence, anisotropy, dispersion)
// show in nothing else:
//
//   lit      the loader's own material, as loaded, under a fixed image-based
//            light (LIT_ENVIRONMENT): the RoomEnvironment PMREM as the scene
//            environment, NoToneMapping, a fixed exposure. Only a loader that
//            builds a MeshPhysicalMaterial (GLTFLoader, USDLoader) can show a
//            layer, so verify.mjs asks for it there alone.
//   lit_flat the same under a uniform white environment: the expected picture
//            usdrecord's dome-lit render is compared with (verify.mjs,
//            usdOracle), since a dome light is a uniform environment.
//   lit_coat `lit` with only the meshes three.js's USDLoader can show lit
//            drawn (usdLitJudgeable): a unit base and emissive colour, no
//            vertex colours, an untextured coat. The USD targets' lit judge.
//
// A file's own lights and cameras (mesh-gaps item 5) have modes of their own,
// listed at CAMLIGHT below: `camera` and `camera_cover` (through the file's
// first camera), `lit_lights` (lit by the file's lights alone) and the
// per-light `light:<node>` pictures of the cross-renderer lights judge.
//
// A file's points and lines (mesh-gaps item 10) draw in every mode, as
// THREE.Points (4 px, sizeAttenuation off) and THREE.LineSegments (1 px), with
// materials of this page's own (WIRE below): the loaders' differ (GLTFLoader's
// PointsMaterial is 1 px, OBJLoader's 10 px beside an MTL) and the expected
// picture is GLTFLoader's.
//
// Ruling IDs ("Ruling A35") are mesh-gaps item 4's rulings, listed with
// their costs in the status block of
// docs/superpowers/plans/2026-11-04-mesh-material-layers.md.

import * as THREE from 'three';
import { RoomEnvironment } from 'three/addons/environments/RoomEnvironment.js';
import { ColorEnvironment } from 'three/addons/environments/ColorEnvironment.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { DRACOLoader } from 'three/addons/loaders/DRACOLoader.js';
import { MeshoptDecoder } from 'three/addons/libs/meshopt_decoder.module.js';
import { KTX2Loader } from 'three/addons/loaders/KTX2Loader.js';
import { OBJLoader } from 'three/addons/loaders/OBJLoader.js';
import { MTLLoader } from 'three/addons/loaders/MTLLoader.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { PLYLoader } from 'three/addons/loaders/PLYLoader.js';
import { FBXLoader } from 'three/addons/loaders/FBXLoader.js';
import { ColladaLoader } from 'three/addons/loaders/ColladaLoader.js';
import { ThreeMFLoader } from 'three/addons/loaders/3MFLoader.js';
import { USDLoader } from 'three/addons/loaders/USDLoader.js';
import { depthFirstRanks, unnamedKey } from './node-rank.js';
import { cameraViewFor, externalPictureSize, UNSET_ASPECT } from './camera-view.js';

const SIZE = 256;

const renderer = new THREE.WebGLRenderer({ antialias: false, preserveDrawingBuffer: true, alpha: true });
renderer.setPixelRatio(1);
renderer.setSize(SIZE, SIZE);
renderer.setClearColor(0x000000, 0);
renderer.outputColorSpace = THREE.SRGBColorSpace;
renderer.toneMapping = THREE.NoToneMapping;
document.body.appendChild(renderer.domElement);

// The `lit` mode's light: three.js's RoomEnvironment (r183, the model-viewer
// room: a point light, six boxes and seven area lights in a box) prefiltered
// by PMREMGenerator.fromScene with its documented blur (sigma 0.04), as the
// scene environment. No tone mapping (renderer.toneMapping stays
// NoToneMapping, so the exposure is `scene.environmentIntensity`, LIT_EXPOSURE,
// not `toneMappingExposure`, which only a tone-mapping pass applies). 0.5 is
// the brightest exposure at which the room does not clip the `layered`
// fixture (scanned on its six-box version: 0.65 % of its pixels at 0.5, 47 %
// at 1.0; a clipped pixel hides a changed layer; the final seven-box fixture
// clips 1 pixel at 0.5), and the one at which two expected files with
// different layer sets differ most. The `lit_flat` mode's light is a uniform
// white environment (ColorEnvironment, 0xffffff), what a constant-colour
// dome light is, at 1.0, where nothing clips (its brightest pixel is 242).
// `lit_coat` is exposed for a coat alone: a clearcoat over a black base
// reflects F0 0.04 of the room. Scanned 2026-11-05 on `layered`'s lacquer
// through USDLoader at the then 20/255 tolerance: at 0.5 a coat of 1 against
// one of 0.2 moves no pixel past it (max delta 20); at 4 they differ on 38 %
// of the coat's pixels (max delta 47) and nothing clips (brightest pixel
// 184). At LIT_TOLERANCE 1 and exposure 4, a removed clearcoat moves 100 %
// of the coat's pixels (max delta 63) and a removed ior 100 % (max delta 6).
const LIT_EXPOSURE = 0.5;
const FLAT_EXPOSURE = 1.0;
const LIT_COAT_EXPOSURE = 4.0;
const pmrem = new THREE.PMREMGenerator(renderer);
const LIT_ENVIRONMENT = pmrem.fromScene(new RoomEnvironment(), 0.04).texture;
const FLAT_ENVIRONMENT = pmrem.fromScene(new ColorEnvironment(0xffffff)).texture;
const LIT_MODES = {
  lit: { environment: LIT_ENVIRONMENT, exposure: LIT_EXPOSURE },
  lit_flat: { environment: FLAT_ENVIRONMENT, exposure: FLAT_EXPOSURE },
  lit_coat: { environment: LIT_ENVIRONMENT, exposure: LIT_COAT_EXPOSURE },
};
// The lit modes' per-channel tolerance. A correct lit case is bit-exact (the
// same loader, renderer, environment and data on both sides: max delta 0 on
// every judged case of three runs), where the unlit modes' 20/255 absorbs
// two loaders' texture filtering. 20 hid four layers: removing anisotropy,
// specular, ior or dispersion from the expected file moved no pixel past it
// (max deltas 12, 13, 3 and 0, review of 2026-11-05). 1 absorbs the 1/255
// rasterisation noise a differently ordered geometry can give (a PLY-sourced
// GLB read max delta 1 against its expected picture in round 0) and sees
// everything above it. Calibrated 2026-11-05 on the final `layered`: self
// and GLB -> GLB 0 pixels at 0, 1 and 2. What each layer moves is measured
// per expected file (layerVisibility) and recorded in the report.
const LIT_TOLERANCE = 1;

// The meshes three.js's USDLoader can show lit, judged on the material AS
// LOADED on each side (the expected GLB through GLTFLoader, the output
// through its own loader): a base colour and an emissive colour of 0 or 1 per
// channel (USDComposer decodes both as sRGB, USDComposer.js:2512-2520,
// :2541-2562, which agrees with linear there alone), no vertex colours (it
// keeps displayColor[0], :1089-1096), and a coat without maps (it reads
// `inputs:clearcoat` and `inputs:clearcoatRoughness` as values, :2661-2671).
// Both sides draw exactly these meshes in `lit_coat`, so a colour written
// wrongly hides a mesh on one side and fails the comparison on coverage.
// Whether an image holds an alpha below 1 anywhere (stats.textureAlpha),
// once per image.
const imageAlpha = new WeakMap();
function imageHasAlpha(image) {
  if (!image || !image.width || !image.height) return false;
  if (imageAlpha.has(image)) return imageAlpha.get(image);
  let has = false;
  try {
    const c = document.createElement('canvas');
    c.width = image.width;
    c.height = image.height;
    const ctx = c.getContext('2d');
    ctx.drawImage(image, 0, 0);
    const d = ctx.getImageData(0, 0, c.width, c.height).data;
    for (let i = 3; i < d.length; i += 4) {
      if (d[i] < 255) {
        has = true;
        break;
      }
    }
  } catch {
    has = false;
  }
  imageAlpha.set(image, has);
  return has;
}

const unit = (c) => [c.r, c.g, c.b].every((v) => v === 0 || v === 1);
function usdLitJudgeable(mesh) {
  const orig = mesh.userData.original ?? mesh.material;
  const materials = Array.isArray(orig) ? orig : [orig];
  if (mesh.userData.noMaterial || mesh.geometry.attributes.color) return false;
  return materials.every(
    (m) => m && (!m.color || unit(m.color)) && (!m.emissive || unit(m.emissive)) && !m.clearcoatMap && !m.clearcoatRoughnessMap && !m.clearcoatNormalMap,
  );
}

// R = u, G = v (both fract'ed by RepeatWrapping), B = a 8x8 checker so a
// mirrored or rotated mapping is visible even where the gradients look alike.
function uvTexture() {
  const n = 256;
  const data = new Uint8Array(n * n * 4);
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      const i = (y * n + x) * 4;
      data[i] = x;
      data[i + 1] = y;
      data[i + 2] = ((x >> 5) + (y >> 5)) % 2 ? 200 : 40;
      data[i + 3] = 255;
    }
  }
  const t = new THREE.DataTexture(data, n, n, THREE.RGBAFormat);
  t.wrapS = t.wrapT = THREE.RepeatWrapping;
  t.magFilter = THREE.NearestFilter;
  t.minFilter = THREE.NearestFilter;
  t.generateMipmaps = false;
  t.colorSpace = THREE.NoColorSpace;
  t.needsUpdate = true;
  return t;
}
const UV_TEXTURE = uvTexture();
// Sample at texel CENTRES where files put their UVs most (0, 1/2, 1, every
// multiple of 1/8 where the checker turns), not on texel edges. A triangle
// lying exactly on u = 1 (a box face mapped to the square's edge) otherwise
// picks texel 255 or texel 0 pixel by pixel from sub-ulp rasterization noise
// — R jumps by 255, the checker flips — so two renders of geometry equal to
// 1e-7 (one skeleton through GLTFLoader, an equivalent one through
// FBXLoader) disagree on whole faces. Both sides of every comparison sample
// the same shifted texture.
UV_TEXTURE.offset.set(0.5 / 256, 0.5 / 256);

// The set selector: `uv1`, `uv2` and `uv3` draw the same gradient through uv
// sets 1–3, by the texture's `channel`. three.js's built-in materials address
// four sets and no more: getChannel names channel n `uv${n}`, and the vertex
// shader declares only `uv` to `uv3`
// (node_modules/three/build/three.module.js:7460-7468, :6904-6922, :7795-7797).
// verify.mjs asks for a per-set mode only where the expected file has that
// set, and skips sets past the fourth with that citation.
const UV_SET_MODES = ['uv', 'uv1', 'uv2', 'uv3'];
const UV_SET_TEXTURES = UV_SET_MODES.map((_, set) => {
  if (set === 0) return UV_TEXTURE;
  const t = UV_TEXTURE.clone();
  t.channel = set;
  return t;
});
// The attribute a loader puts uv set n in: `uv`, `uv1`, … (GLTFLoader names a
// fifth set and later `texcoord_4`, …, GLTFLoader.js:2221-2224, :4789).
const uvAttribute = (set) => (set === 0 ? 'uv' : `uv${set}`);
function uvSetCount(root) {
  let n = 0;
  for (const m of meshes(root)) {
    const a = m.geometry.attributes;
    for (let set = 0; set < 8; set++) {
      if (a[uvAttribute(set)] || a[`texcoord_${set}`]) n = Math.max(n, set + 1);
    }
  }
  return n;
}

// Which loaders hand back UVs with V pointing UP (OBJ/FBX/Collada/3MF/PLY's
// convention) rather than glTF's V DOWN. Their textures compensate with
// flipY; the uv mode has to compensate the same way to compare like with like.
// 3MF, checked against r183: a tex2coord's origin is the image's lower-left
// (3MF Materials §3.1); ThreeMFLoader copies u and v into the uv attribute
// unchanged (buildTexturedMesh) and loads the image with TextureLoader without
// touching flipY, which three defaults to true (Texture.js) — V runs up.
// USD belongs here too: USDComposer loads `primvars:st` unchanged (USD's
// origin is the bottom-left) and creates every texture with `new Texture()`,
// whose flipY defaults to true — the same compensation OBJ makes.
const V_UP = { obj: true, fbx: true, dae: true, '3mf': true, ply: true, stl: true, usdz: true, usda: true, usdc: true, usd: true };

// ─── three.js r183 USDLoader: documented limitations ─────────────────────────
// Each is proven by source and by a minimal file (see verify.mjs, USD oracle
// notes); the modes they break are verified by usdrecord instead.
//  - albedo: USDComposer._applyPreviewSurface reads `color3f
//    inputs:diffuseColor` (and a UsdUVTexture's scale) with
//    `setRGB(..., SRGBColorSpace)`, but USD colours are linear (its own
//    USDZExporter writes linear `material.color`), and _buildMesh uses only
//    `primvars:displayColor[0]` as a constant, so per-vertex colour is lost.
//    Neither has anything to act on when the expected scene has no vertex
//    colours and every base colour is 0 or 1 in each channel, where sRGB and
//    linear agree (`usdAlbedoJudgeable`): there, albedo is compared like any
//    mode, so a USDZ's base-colour texture is judged here too. Unless that
//    texture samples a set past the second: USDComposer gives a texture
//    channel 1 or 2 only for the varnames `st1` and `st2`, and 0 for any
//    other (USDComposer.js:3130, :3159-3171, :3201), and builds no uv
//    attribute past `uv1` (:1371-1398, :1778-1784).
//  - skinned, animated: USDAParser._parseAttributeValue reorders a `quatf`
//    from text order (w, x, y, z) but its array branch never does, so every
//    SkelAnimation rotation in USDA text is read as (x, y, z, w) = garbage.
//    (USDC is read correctly; verify.mjs renders a usdcat crate copy.)
//  - morph targets: USDComposer has no BlendShape support at all, so even the
//    default pose (default weights) is wrong.
//  - a joint whose rest scale is zero: USDComposer._buildSkeleton
//    (USDComposer.js:3446) decomposes each restTransform with
//    Matrix4.decompose, which returns unit scale and identity rotation for any
//    singular matrix (src/math/Matrix4.js, `if ( det === 0 )`), so the rest
//    pose shows a joint USD (and glTF) collapse at full size.
const USD_TEXT_ROOT = { usdz: true, usda: true };
const USD_ANY = { usdz: true, usda: true, usdc: true, usd: true };
// Whether the USD albedo defects above can act on a scene, from its expected
// picture's stats. Shared with verify.mjs's SceneKit check
// (window.usdAlbedoJudgeable), whose USD albedo defects (colour values read
// as Display P3, primvar readers skipped) need the same two things.
function usdAlbedoJudgeable(info) {
  return info.vertexColors === false && info.unitColors === true;
}
const USD_ALBEDO_LATE_SET =
  'three.js USDLoader samples a base-colour texture on uv set 2 or later through set 0 or a missing attribute (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:3130, :3159-3171, :3201; no uv2: :1371-1398, :1778-1784); albedo verified by usdrecord and SceneKit';
// The `lit` mode through USDLoader: its colour defect acts on every lit pixel
// of a non-unit base colour, and it reads `inputs:clearcoat`,
// `inputs:clearcoatRoughness` and `inputs:ior` as values only
// (USDComposer.js:2661-2671 and :2626-2630), never through a connection: its
// one connection-following path, _applyTextureOrValue (:2453-2489), serves
// the texture inputs alone, and the clearcoat code does not call it. So a
// coat written through a texture connection is not seen. Pictures stay in
// the report; the usd -> glb cases judge the coat through our reader and
// GLTFLoader, and usdrecord's dome-lit picture is kept beside it (verify.mjs,
// usdOracle, not a judge).
const USD_LIT_SRGB =
  'three.js USDLoader decodes linear UsdPreviewSurface base and emissive colours as sRGB (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:2512-2520 and :2541-2562, setRGB(..., SRGBColorSpace)), so every lit pixel of a colour between 0 and 1 is wrong; the meshes it reads right are judged in lit_coat, the layers by the usd -> glb cases, and usdrecord\'s dome-lit picture is kept';
const USD_LIT_COAT_TEXTURE =
  'three.js USDLoader reads inputs:clearcoat, inputs:clearcoatRoughness and inputs:ior as values only (scripts/mesh-verify/node_modules/three/examples/jsm/loaders/usd/USDComposer.js:2661-2671, :2626-2630) and follows no connection on them (its only connection-following path is _applyTextureOrValue, :2453-2489, which the clearcoat code does not use), so a textured coat is not seen; the coat is judged by the usd -> glb cases, and usdrecord\'s dome-lit picture is kept';
function usdLimitation(format, mode, t, info) {
  if (!USD_TEXT_ROOT[format] && format !== 'usdc' && format !== 'usd') return null;
  if (mode === 'albedo') {
    if (!usdAlbedoJudgeable(info)) {
      return 'three.js USDLoader decodes linear UsdPreviewSurface colours as sRGB and keeps only displayColor[0]; albedo verified by usdrecord';
    }
    if (info.mapSet > 1) return USD_ALBEDO_LATE_SET;
  }
  if (mode === 'lit') {
    // Emissive is decoded as sRGB too (USDComposer.js:2541-2562).
    if (!usdAlbedoJudgeable(info) || info.unitEmissive === false) return USD_LIT_SRGB;
    if (info.coatTextured) return USD_LIT_COAT_TEXTURE;
  }
  // `lit_coat` draws only what USDLoader reads right (usdLitJudgeable), so
  // neither reason above applies to it; the pose limitations below do.
  if (info.morphed) return 'three.js USDLoader has no BlendShape support; verified by usdrecord silhouettes';
  if (info.singularBone && t === 'static') {
    return 'three.js Matrix4.decompose turns a zero-scale USD rest joint into unit scale; verified by usdrecord';
  }
  if (info.skinned && t !== 'static' && USD_TEXT_ROOT[format]) {
    return 'three.js USDAParser does not reorder quatf[] (w,x,y,z); skeletal animation verified by usdrecord and by a usdcat crate copy';
  }
  return null;
}

// A glTF through its own loader decodes KHR_draco_mesh_compression with
// three.js's own Draco decoder (r183's examples/jsm/libs/draco/gltf, the wasm
// build in a worker, GLTFLoader.js:1900-1990) and EXT_ and
// KHR_meshopt_compression with its MeshoptDecoder
// (examples/jsm/libs/meshopt_decoder.module.js, GLTFLoader.js:1584-1660), so
// a compressed original is the native check's reference, read by nothing of
// litegen's (mesh-gaps item 2). Without them GLTFLoader throws on a Draco
// file (:1910-1912) and reads only a meshopt view's fallback buffer
// (:1611-1621). One DRACOLoader keeps its worker pool across loads.
const dracoLoader = new DRACOLoader().setDecoderPath('/three/examples/jsm/libs/draco/gltf/');

// KHR_texture_basisu (mesh-gaps item 3): GLTFLoader refuses a KTX2 texture
// unless setKTX2Loader was called (GLTFLoader.js:1466-1473), and KTX2Loader
// itself refuses to load before detectSupport(renderer) (KTX2Loader.js:361).
// Its transcoder is Binomial's own, r183's examples/jsm/libs/basis/ wasm,
// served from /three/ like the Draco decoder, so a KTX2 original is the
// reference, read by nothing of litegen's. It transcodes each texture to the
// GPU format THIS browser reports (detectSupport reads the renderer's
// extensions; KTX2Loader.js FORMAT_OPTIONS: ASTC or BC7 or BC1/BC3 or ETC2,
// RGBA32 only when none is offered), so a KTX2 original's picture is lossy by
// that format's block compression, where our reading (an exact RGBA8 PNG,
// bit-identical to the same transcoder's RGBA32) is not: the native albedo
// threshold for a KTX2 original is calibrated (verify.mjs KTX2_NATIVE_ALBEDO,
// scripts/mesh-verify/README.md), and `stats.ktx2` records the formats used.
// `ktx2Rgba32` is the same loader told this browser offers no compressed
// format, which leaves its RGBA32 fallback (KTX2Loader.js FORMAT_OPTIONS, the
// uncompressed entries): the transcoder's own RGBA8, which litegen's decoder
// reproduces byte for byte (every_reference_texture_transcodes_bit_exactly),
// so a pair drawn through it is the exact comparison (`setKtx2Mode`).
const ktx2Loader = new KTX2Loader().setTranscoderPath('/three/examples/jsm/libs/basis/').detectSupport(renderer);
const ktx2Rgba32 = new KTX2Loader().setTranscoderPath('/three/examples/jsm/libs/basis/');
ktx2Rgba32.workerConfig = { astcSupported: false, astcHDRSupported: false, etc1Supported: false, etc2Supported: false, dxtSupported: false, bptcSupported: false, pvrtcSupported: false };
let ktx2Mode = 'gpu';
window.setKtx2Mode = (mode) => {
  ktx2Mode = mode;
};
const ktx2Active = () => (ktx2Mode === 'rgba32' ? ktx2Rgba32 : ktx2Loader);
const KTX2_FORMATS = ['RGBAFormat', 'RGBA_BPTC_Format', 'RGBA_ASTC_4x4_Format', 'RGBA_S3TC_DXT1_Format', 'RGBA_S3TC_DXT5_Format', 'RGB_ETC2_Format', 'RGBA_ETC2_EAC_Format', 'RGB_ETC1_Format', 'RGB_PVRTC_4BPPV1_Format', 'RGBA_PVRTC_4BPPV1_Format'];
const ktx2FormatName = (f) => KTX2_FORMATS.find((n) => THREE[n] === f) ?? String(f);

function loaderFor(format) {
  switch (format) {
    case 'glb':
    case 'gltf':
      return new GLTFLoader().setDRACOLoader(dracoLoader).setMeshoptDecoder(MeshoptDecoder).setKTX2Loader(ktx2Active());
    case 'obj':
      return new OBJLoader();
    case 'stl':
      return new STLLoader();
    case 'ply':
      return new PLYLoader();
    case 'fbx':
      return new FBXLoader();
    case 'dae':
      return new ColladaLoader();
    case '3mf':
      return new ThreeMFLoader();
    case 'usdz':
    case 'usda':
    case 'usdc':
    case 'usd':
      return new USDLoader();
    default:
      throw new Error(`no loader for ${format}`);
  }
}

// A PLY file's header text, through `end_header`.
async function plyHeader(url) {
  const head = new TextDecoder('latin1').decode(new Uint8Array(await (await fetch(url)).arrayBuffer()).subarray(0, 8192));
  return head.slice(0, head.indexOf('end_header') + 10);
}

// Load a file and normalise every loader's result to { root, clips }.
async function load(url, format, files) {
  if ((format === 'usda' || format === 'usdc' || format === 'usd') && files && files.length > 1) {
    // A standalone layer's textures are sidecar files. USDLoader.parse
    // composes a standalone layer with no assets and no base path, and
    // USDComposer._loadTexture then asks the LoadingManager to resolve the
    // texture's basename — so the package is handed over exactly there, as
    // the OBJ path hands over its .mtl.
    const base = url.slice(0, url.lastIndexOf('/') + 1);
    const byName = new Map(files.map((f) => [f.split('/').pop(), base + f]));
    const manager = new THREE.LoadingManager();
    manager.setURLModifier((u) => byName.get(u) ?? u);
    const result = await new USDLoader(manager).loadAsync(url);
    return { root: result, clips: result.animations || [] };
  }
  if (format === 'obj') {
    const loader = new OBJLoader();
    const mtl = (files || []).find((f) => f.toLowerCase().endsWith('.mtl'));
    if (mtl) {
      const base = url.slice(0, url.lastIndexOf('/') + 1);
      const ml = new MTLLoader();
      ml.setResourcePath(base);
      const materials = await ml.loadAsync(base + mtl);
      materials.preload();
      loader.setMaterials(materials);
    }
    const root = await loader.loadAsync(url);
    return { root, clips: [] };
  }
  const result = await loaderFor(format).loadAsync(url);
  if (format === 'glb' || format === 'gltf') {
    // `cameras`: the document's cameras in its order (GLTFLoader's
    // `gltf.cameras`), each the object its node placed in the scene;
    // `associations`: each node's object to its index, and `ranks` the
    // depth-first rank of each index in the default scene (lightNodeName).
    const parser = result.parser;
    return { root: result.scene, clips: result.animations || [], cameras: result.cameras || [], associations: parser?.associations, ranks: parser?.json ? depthFirstRanks(parser.json) : undefined, json: parser?.json };
  }
  if (format === 'dae') {
    return { root: result.scene, clips: result.scene.animations || result.animations || [] };
  }
  if (result.isBufferGeometry) {
    // PLYLoader gives a file with no face an unindexed geometry (it sets an
    // index only when a face was read, PLYLoader.js:419-421) and leaves Mesh
    // or Points to the application, so positions with no index draw as
    // Points: the reading SceneKit and assimp give a vertex-only file
    // (research Q6). A file with an `edge` element and no face holds lines,
    // which PLYLoader ignores (handleElement reads `vertex` and `face` alone,
    // PLYLoader.js:480-561): it draws nothing, not its vertices as points.
    if (format === 'ply' && !result.index) {
      const root = new THREE.Group();
      if (!/^element edge\s+[1-9]/m.test(await plyHeader(url))) {
        const points = new THREE.Points(result, new THREE.PointsMaterial({ color: 0xffffff, size: POINT_SIZE, sizeAttenuation: false }));
        points.userData.noMaterial = true;
        root.add(points);
      }
      return { root, clips: [] };
    }
    // STL and PLY hand back bare geometry and no material at all.
    const mesh = new THREE.Mesh(result, new THREE.MeshBasicMaterial({ color: 0xffffff }));
    mesh.userData.noMaterial = true;
    return { root: mesh, clips: [] };
  }
  if (format === '3mf') {
    // 3MF is Z-up — "the z-axis increasing to the top of the output field"
    // (3MF Core §3.1) — and ThreeMFLoader returns the model in the file's own
    // frame, leaving the turn to the application. Its documentation does
    // exactly this: `object.rotation.set( - Math.PI / 2, 0, 0 ); // z-up
    // conversion` (examples/jsm/loaders/3MFLoader.js). The scenes compared
    // here are glTF's +Y up. The loader reads `unit` and does not scale, so
    // there is nothing to undo for units.
    const up = new THREE.Group();
    up.rotation.x = -Math.PI / 2;
    up.add(result);
    return { root: up, clips: [] };
  }
  return { root: result, clips: result.animations || [] };
}

// Wait until every texture any material references has decoded. GLTFLoader
// resolves only once its textures are ready, but MTLLoader, FBXLoader,
// ColladaLoader and the 3MF loader hand back the model first and fill textures
// in asynchronously — rendering at that moment captures untextured surfaces,
// a race that made identical files pass in one run and fail in the next.
const MAP_KEYS = ['map', 'emissiveMap', 'normalMap', 'alphaMap', 'bumpMap', 'specularMap', 'aoMap', 'metalnessMap', 'roughnessMap'];
async function texturesReady(root, timeoutMs = 15000) {
  const start = performance.now();
  for (;;) {
    let pending = 0;
    root.traverse((o) => {
      if (!o.isMesh && !isWire(o)) return;
      for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
        if (!m) continue;
        for (const k of MAP_KEYS) {
          const t = m[k];
          if (!t || !t.isTexture) continue;
          const im = t.image;
          const ready = im && (im.data || (im.width > 0 && im.complete !== false));
          if (!ready) pending++;
        }
      }
    });
    if (!pending) return true;
    if (performance.now() - start > timeoutMs) return false;
    await new Promise((r) => setTimeout(r, 25));
  }
}

function meshes(root) {
  const out = [];
  root.traverse((o) => {
    if (o.isMesh) out.push(o);
  });
  return out;
}

// ─── points and lines (mesh-gaps item 10) ───────────────────────────────────
//
// WIRE. `meshes()` stays triangles alone: every statistic of the triangles
// (normals, uv sets, skins, morph targets, layers) is theirs. `drawables()` adds
// the objects a loader builds for points (THREE.Points) and lines (Line,
// LineSegments, LineLoop; `isLine` holds for all three), which every mode
// draws with a material of its own, never the loader's:
//
//  - Points are `PointsMaterial` at POINT_SIZE px with `sizeAttenuation` off, and
//    lines `LineBasicMaterial` (1 px): GLTFLoader's points are 1 px, OBJLoader's
//    10 px beside an MTL material, and the expected picture is GLTFLoader's.
//  - albedo, albedo_novc and albedo_map: the source material's colour, map and
//    vertex colours, as `albedoMaterial` does for a mesh. r183 samples a point's
//    `map` through its `uv` attribute when the geometry has one
//    (WebGLPrograms.js:300 `pointsUvs`: Points, a `uv` attribute and a map,
//    which defines USE_POINTS_UV; points.glsl.js:12-23), and across the point's
//    own sprite otherwise (map_particle_fragment.glsl.js): a textured points
//    primitive's texture is drawn.
//  - cover, camera_cover, wire_cover_points and wire_cover_lines: white.
//  - every other mode (geom, camera, normals, uv…, lit…): one fixed colour, so a
//    picture of the data does not depend on which loader built the object.
//
// `wire_cover_points` is the points alone, white, and `wire_cover_lines` the
// lines alone: the expected pictures usdrecord's and SceneKit's wire pictures
// are compared with (compareWire), each kind apart, as a point's footprint
// differs by renderer (Storm's are a pixel or so) where its place does not, and
// a missing point must not hide behind the lines. `blind` draws the expected scene
// as a reader that cannot read all of it would draw it (verify.mjs,
// wirePlan): 'linecolors' with its lines' vertex colours left out, 'lines'
// without its lines, 'wire' without its points and lines. The triangle
// comparison stays whole. The `light` modes draw neither points nor lines
// (nothing lights them).
const POINT_SIZE = 4;
const WIRE_COLOR = 0xffa000;
const WIRE_BLINDS = ['linecolors', 'lines', 'wire'];
const isWire = (o) => o.isPoints === true || o.isLine === true;
function drawables(root) {
  const out = [];
  root.traverse((o) => {
    if (o.isMesh || isWire(o)) out.push(o);
  });
  return out;
}
function wireCounts(root) {
  // `vertices`: the points drawn (an indexed primitive draws its index's).
  const n = { points: 0, lines: 0, vertices: 0 };
  for (const o of drawables(root)) {
    if (!isWire(o)) continue;
    const g = o.geometry;
    const count = g.index ? g.index.count : g.attributes.position?.count;
    if (!count) continue;
    n[o.isPoints ? 'points' : 'lines']++;
    if (o.isPoints) n.vertices += count;
  }
  return n;
}
function wireMaterial(obj, src, kind, blind) {
  const make = (props) =>
    obj.isPoints ? new THREE.PointsMaterial({ size: POINT_SIZE, sizeAttenuation: false, ...props }) : new THREE.LineBasicMaterial(props);
  switch (kind) {
    case 'albedo':
    case 'albedo_novc':
    case 'albedo_map': {
      const hasColors = kind !== 'albedo_novc' && !(blind === 'linecolors' && obj.isLine) && !!obj.geometry.attributes.color;
      if (obj.userData.noMaterial || !src) return make({ color: 0xffffff, vertexColors: hasColors });
      return make({
        color: src.color && !(kind === 'albedo_map' && src.map) ? src.color.clone() : new THREE.Color(0xffffff),
        map: src.map || null,
        vertexColors: hasColors,
      });
    }
    case 'cover':
    case 'camera_cover':
    case 'wire_cover_points':
    case 'wire_cover_lines':
      return make({ color: 0xffffff });
    default:
      return make({ color: WIRE_COLOR });
  }
}
// Whether `o` is drawn in `mode`: the triangles never in the wire pictures,
// `lit_coat` only the meshes USDLoader reads right, and the points and lines
// never in a `light` picture or `lit_coat`, nor where `blind` leaves them out.
function shown(o, mode, blind) {
  const wire = isWire(o);
  if (mode === 'wire_cover_points') return o.isPoints === true;
  if (mode === 'wire_cover_lines') return o.isLine === true;
  if (!wire) return mode !== 'lit_coat' || usdLitJudgeable(o);
  if (mode === 'lit_coat' || isLightMode(mode)) return false;
  return !(blind === 'wire' || (blind === 'lines' && o.isLine === true));
}

// The layers a loaded material carries, by glTF's names: each a
// MeshPhysicalMaterial property away from its default (GLTFLoader sets them
// from the KHR_materials_* extensions, USDLoader from UsdPreviewSurface's
// clearcoat, clearcoatRoughness and ior).
function materialLayers(m) {
  if (!m || !m.isMeshPhysicalMaterial) return [];
  const out = [];
  if (m.clearcoat > 0 || m.clearcoatMap) out.push('clearcoat');
  if (m.transmission > 0 || m.transmissionMap) out.push('transmission');
  if (m.thickness > 0 || m.thicknessMap || Number.isFinite(m.attenuationDistance)) out.push('volume');
  if (m.ior !== 1.5) out.push('ior');
  if (m.specularIntensity !== 1 || m.specularIntensityMap || m.specularColorMap || !(m.specularColor.r === 1 && m.specularColor.g === 1 && m.specularColor.b === 1)) out.push('specular');
  if (!(m.sheenColor.r === 0 && m.sheenColor.g === 0 && m.sheenColor.b === 0) || m.sheenColorMap) out.push('sheen');
  if (m.iridescence > 0 || m.iridescenceMap) out.push('iridescence');
  if (m.anisotropy > 0 || m.anisotropyMap) out.push('anisotropy');
  if (m.dispersion > 0) out.push('dispersion');
  return out;
}

function albedoMaterial(src, mesh, useVertexColors = true, mapAlone = false) {
  const hasColors = useVertexColors && !!mesh.geometry.attributes.color;
  if (mesh.userData.noMaterial || !src) {
    return new THREE.MeshBasicMaterial({ color: 0xffffff, vertexColors: hasColors, side: THREE.FrontSide });
  }
  return new THREE.MeshBasicMaterial({
    color: src.color && !(mapAlone && src.map) ? src.color.clone() : new THREE.Color(0xffffff),
    map: src.map || null,
    vertexColors: hasColors,
    side: THREE.FrontSide,
  });
}

// V-up UVs are flipped into glTF's convention for the uv mode ONLY, and put
// back for every other mode: the loader's own textures compensate for V-up
// with flipY, so albedo must sample the UVs as loaded. Flipping in place and
// never restoring made every frame rendered after the first uv frame — every
// animation sample of a V-up, animated, textured file — sample its texture
// upside down. The state is kept per uv ATTRIBUTE, not per mesh: loaders share
// one BufferGeometry between instances (ColladaLoader caches each geometry's
// build), and per-mesh state flips a shared attribute twice, i.e. not at all.
// Each uv mode flips the set it draws (`name`), and only while it draws it.
const flippedUv = new WeakSet();
function setUvFlip(mesh, flipped, name = 'uv') {
  const uv = mesh.geometry.attributes[name];
  if (!uv || flippedUv.has(uv) === flipped) return;
  const a = uv.array;
  for (let i = 1; i < a.length; i += 2) a[i] = 1 - a[i];
  uv.needsUpdate = true;
  if (flipped) flippedUv.add(uv);
  else flippedUv.delete(uv);
}

function applyMode(root, mode, format, blind) {
  // Every `light:<node>` picture draws the same white Lambert surfaces.
  const kind = isLightMode(mode) ? 'light' : mode;
  // Points and lines too (WIRE): a point or a line with uvs flips them like a mesh.
  for (const mesh of drawables(root)) {
    if (V_UP[format]) UV_SET_MODES.forEach((m, set) => setUvFlip(mesh, mode === m, uvAttribute(set)));
    if (!mesh.userData.original) mesh.userData.original = mesh.material;
    const orig = mesh.userData.original;
    const pick = (fn) => (Array.isArray(orig) ? orig.map((m) => fn(m)) : fn(orig));
    mesh.userData.modeMaterials = mesh.userData.modeMaterials || {};
    if (mesh.userData.modeMaterials[kind]) {
      mesh.material = mesh.userData.modeMaterials[kind];
      continue;
    }
    if (isWire(mesh)) {
      mesh.material = pick((m) => wireMaterial(mesh, m, kind, blind));
      mesh.userData.modeMaterials[kind] = mesh.material;
      continue;
    }
    switch (kind) {
      // `camera` is the geom mode through the file's own camera.
      case 'geom':
      case 'camera':
        mesh.material = pick(() => new THREE.MeshNormalMaterial({ flatShading: true, side: THREE.FrontSide }));
        break;
      // One light alone on white Lambert surfaces (CAMLIGHT, below).
      case 'light':
        mesh.material = pick(() => new THREE.MeshLambertMaterial({ color: 0xffffff, side: THREE.FrontSide }));
        break;
      case 'normals':
        mesh.material = pick(() => new THREE.MeshNormalMaterial({ flatShading: false, side: THREE.FrontSide }));
        break;
      case 'albedo':
        mesh.material = pick((m) => albedoMaterial(m, mesh));
        break;
      // Albedo with vertex colours left out, for the one native comparison
      // where three.js's vertex colours are known to be wrong (verify.mjs).
      case 'albedo_novc':
        mesh.material = pick((m) => albedoMaterial(m, mesh, false));
        break;
      // Albedo with every mapped colour left to its map, for FBX originals:
      // three.js multiplies a colour into the map beside it (verify.mjs).
      case 'albedo_map':
        mesh.material = pick((m) => albedoMaterial(m, mesh, true, true));
        break;
      case 'cover':
      case 'camera_cover':
      case 'wire_cover_points':
      case 'wire_cover_lines':
        // Coverage only, both sides: what usdrecord's alpha shows (Storm
        // draws back faces; the other modes cull them).
        mesh.material = pick(() => new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide }));
        break;
      case 'lit':
      case 'lit_flat':
      case 'lit_coat':
      case 'lit_lights':
        // The loader's own material, every layer it built included; the
        // light is the scene environment (renderFile).
        mesh.material = orig;
        break;
      case 'uv': {
        const uv = mesh.geometry.attributes.uv;
        mesh.material = pick(() => new THREE.MeshBasicMaterial({ map: uv ? UV_TEXTURE : null, color: uv ? 0xffffff : 0xff00ff, side: THREE.FrontSide }));
        break;
      }
      // The set selector: the same picture through set 1, 2 or 3. A mesh
      // without that set draws magenta, as `uv` does without set 0.
      case 'uv1':
      case 'uv2':
      case 'uv3': {
        const set = UV_SET_MODES.indexOf(mode);
        const uv = mesh.geometry.attributes[uvAttribute(set)];
        mesh.material = pick(() => new THREE.MeshBasicMaterial({ map: uv ? UV_SET_TEXTURES[set] : null, color: uv ? 0xffffff : 0xff00ff, side: THREE.FrontSide }));
        break;
      }
    }
    mesh.userData.modeMaterials[kind] = mesh.material;
  }
}

// Every picture is SIZE x SIZE but the `camera` modes', which are drawn at the
// file camera's own aspect (cameraViewOf).
function capture(w = SIZE, h = SIZE) {
  const gl = renderer.getContext();
  const px = new Uint8Array(w * h * 4);
  gl.readPixels(0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, px);
  // readPixels is bottom-up; flip to top-down for images and diffs.
  const out = new Uint8ClampedArray(w * h * 4);
  for (let y = 0; y < h; y++) {
    out.set(px.subarray((h - 1 - y) * w * 4, (h - y) * w * 4), y * w * 4);
  }
  return out;
}

function toPng(pixels, w = SIZE, h = SIZE) {
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const ctx = c.getContext('2d');
  ctx.putImageData(new ImageData(pixels, w, h), 0, 0);
  return c.toDataURL('image/png');
}

// Pixel comparison. A pixel differs when coverage differs, or when any
// channel differs by more than `tol` (8-bit). The ratio is over the union of
// covered pixels, so a small object is judged on itself, not on background.
function compare(a, b, tol = 20) {
  let covered = 0;
  let differ = 0;
  let maxDelta = 0;
  const diff = new Uint8ClampedArray(a.length);
  for (let i = 0; i < a.length; i += 4) {
    const ca = a[i + 3] > 0;
    const cb = b[i + 3] > 0;
    if (!ca && !cb) continue;
    covered++;
    let d = 0;
    if (ca !== cb) d = 255;
    else d = Math.max(Math.abs(a[i] - b[i]), Math.abs(a[i + 1] - b[i + 1]), Math.abs(a[i + 2] - b[i + 2]));
    maxDelta = Math.max(maxDelta, d);
    if (d > tol) {
      differ++;
      diff[i] = 255;
      diff[i + 1] = 0;
      diff[i + 2] = 60;
      diff[i + 3] = 255;
    } else {
      const g = 70 + (a[i] + a[i + 1] + a[i + 2]) / 12;
      diff[i] = g;
      diff[i + 1] = g;
      diff[i + 2] = g;
      diff[i + 3] = 255;
    }
  }
  return { covered, differ, ratio: covered ? differ / covered : 0, maxDelta, diff };
}

const expectedCache = new Map();
const expectedInfo = new Map();

// A camera that frames whatever was loaded, from the same direction the fixed
// cameras use. For third-party files only: two loaders that disagree about
// units (FBX's UnitScaleFactor, 3MF's unit) still produce comparable pictures,
// while a wrong rotation, a missing part or a flipped face still shows.
// `leaveOut`: objects the frame ignores, as a reader that never builds them
// would frame the file (the `wire` blind: three.js's USDLoader drops Points
// and BasisCurves, so its picture of an original is framed on the meshes).
function frameCamera(root, leaveOut = []) {
  root.updateMatrixWorld(true);
  const box = new THREE.Box3();
  if (!leaveOut.length) box.setFromObject(root, true);
  else {
    const v = new THREE.Vector3();
    root.traverse((o) => {
      const position = o.geometry?.getAttribute?.('position');
      if (!position || leaveOut.includes(o)) return;
      for (let i = 0; i < position.count; i++) {
        if (o.isMesh) o.getVertexPosition(i, v);
        else v.fromBufferAttribute(position, i);
        box.expandByPoint(v.applyMatrix4(o.matrixWorld));
      }
    });
  }
  const c = box.getCenter(new THREE.Vector3());
  const r = Math.max(box.getSize(new THREE.Vector3()).length() / 2, 1e-6);
  const fov = 40;
  const dist = (r / Math.sin(THREE.MathUtils.degToRad(fov / 2))) * 1.02;
  const d = new THREE.Vector3(1, 0.7, 1.5).normalize();
  return {
    fov,
    position: c.clone().addScaledVector(d, dist).toArray(),
    target: c.toArray(),
    near: Math.max(dist - r * 1.5, dist * 0.01),
    far: dist + r * 1.5,
  };
}

// ─── lights and cameras (mesh-gaps item 5) ──────────────────────────────────
//
// CAMLIGHT. The modes that judge a file's own lights and cameras:
//
//   camera        the geom mode through the file's own first camera (the
//                 loader's: GLTFLoader's `gltf.cameras[0]`), at rest and at
//                 the sample times, drawn SIZE high and SIZE x the camera's
//                 aspect wide (cameraViewOf), so a wrong axis, fov, aspect or
//                 near plane moves every pixel. A case is drawn at the
//                 expected picture's size, so a wrong aspect squeezes it.
//   camera_cover  coverage, both sides, through the same camera: what
//                 usdrecord --camera shows (verify.mjs).
//   lit_lights    the loader's own material lit by the file's own lights and
//                 nothing else (no environment), through LinearToneMapping at
//                 LIT_LIGHTS_EXPOSURE 0.4, a pure scale, and the
//                 fixture's camera: item 4's `lit` with only the file's
//                 lights, as the spec asks. Judged at the lit tolerance.
//   light:<node>  one light alone, the one on glTF node <node>, on white
//                 Lambert surfaces, through the fixture's camera, at an
//                 exposure that puts its brightest pixel near 230
//                 (LIGHT_TARGET): the expected pictures of the cross-renderer
//                 lights judge (compareLightPicture), drawn for expected files
//                 alone (`lights_each`), as usdrecord's Storm draws a USD
//                 light: with no `range` cutoff, which glTF alone holds
//                 (item 5's Ruling R27: no native Collada or USD form, so
//                 other readers light past it; GLTFLoader makes the range
//                 three.js's `distance`, whose window
//                 `pow2(saturate(1 - pow4(d / range)))` is left out here:
//                 getDistanceAttenuation, three/src/renderers/shaders/
//                 ShaderChunk/lights_pars_begin.glsl.js L56-L69).
//   lightsk:<node> the same light as SceneKit draws a Collada one (the
//                 Collada judge's expected pictures): no range cutoff
//                 (R27); for a point or spot light no distance falloff at
//                 all, three.js's `decay` 0 (SceneKit reads <quadratic_
//                 attenuation> as no attenuation, start and end distance 0,
//                 and lights a surface as brightly near the light as far:
//                 measured, verify.mjs SCENEKIT_NO_FALLOFF); and its colour
//                 as SceneKit reads <color>, litegen's linear colour ×
//                 intensity (R17) decoded as sRGB, each channel cut at 1
//                 (item 5's Ruling R26, measured: SCENEKIT_SRGB_COLOR).
//
// `external` (the FBX judge, verify.mjs): a camera and lights built from
// assimp's reading of a file (camlight-oracle.mjs, assimpView), each with a
// pose per time key, drawn in place of the file's own, which are hidden.
const CAMERA_MODES = { camera: true, camera_cover: true };
const isLightMode = (m) => m.startsWith('light:') || m.startsWith('lightsk:');
// A light mode's node, and whether it is SceneKit's variant.
const lightModeNode = (m) => m.slice(m.indexOf(':') + 1);
const isSceneKitLightMode = (m) => m.startsWith('lightsk:');
const LIGHT_TARGET = 230;
// `lit_lights` is drawn through LinearToneMapping (a pure scale before the
// sRGB encoding) at this exposure: a clipped pixel hides a changed light.
// Scanned 2026-11-06 on `staged`'s expected file, at rest and at its six
// sample times: at 1.0, 3,718-4,773 of its 12,450 pixels clip, at 0.8
// 494-737, at 0.6 134-178, at 0.5 0-19, at 0.4 none (brightest 243), at 0.3
// none (brightest 214). 0.4 is the brightest that does not clip.
const LIT_LIGHTS_EXPOSURE = 0.4;
function cameraViewOf(cam) {
  return cameraViewFor(cam.isOrthographicCamera ? (cam.right - cam.left) / (cam.top - cam.bottom) : cam.aspect, SIZE);
}
// The glTF node a light or a camera hangs on: GLTFLoader names a light-only
// or camera-only node's object after the node, and keeps a node's own name in
// `userData.name` (GLTFLoader.js, loadNode, L4355-L4358). A node with no name
// is `node ${rank}`, its depth-first rank in the document's default scene
// (page/node-rank.js; the node's index comes from `associations`, the
// parser's map of each node's object to `{ nodes: i }`, L4394-L4405, and a
// clone's own, `_getNodeRef` L2791-L2814, and `ranks` maps it on), as
// camlight-oracle.mjs names it (item 5's Ruling R25 as amended 2026-11-08:
// litegen numbers a scene's nodes depth-first, so a file in another order
// is paired by rank, not by index). Every judge picks a camera by this name,
// never by its place in a reader's list (verify.mjs, `info.cameras[0]`).
function lightNodeName(obj, associations, ranks) {
  for (let o = obj; o; o = o.parent) {
    if (o.userData?.name !== undefined) return o.userData.name;
    const i = associations?.get(o)?.nodes;
    if (i !== undefined) return unnamedKey(ranks, i);
  }
  return obj.name;
}
const lightKind = (l) => (l.isDirectionalLight ? 'directional' : l.isSpotLight ? 'spot' : l.isPointLight ? 'point' : l.type);
function externalCamera(spec) {
  const cam = new THREE.PerspectiveCamera(THREE.MathUtils.radToDeg(spec.yfov), spec.aspect > 0 ? spec.aspect : 1, spec.znear, spec.zfar);
  cam.userData.frames = spec.frames;
  return cam;
}
function placeCamera(cam, key) {
  const f = cam.userData.frames?.[key] ?? cam.userData.frames?.static;
  if (!f) return;
  cam.position.fromArray(f.eye);
  cam.up.fromArray(f.up);
  cam.lookAt(new THREE.Vector3().fromArray(f.eye).add(new THREE.Vector3().fromArray(f.forward)));
  cam.updateMatrixWorld(true);
}
// A light as GLTFLoader builds one from KHR_lights_punctual (GLTFLoader.js
// L690-L752: linear colour, `distance` = range, a spot's angle the outer cone
// and penumbra 1 - inner/outer, three.js's default decay of 2).
function externalLight(spec) {
  const color = new THREE.Color().setRGB(spec.color[0], spec.color[1], spec.color[2], THREE.LinearSRGBColorSpace);
  let l;
  if (spec.kind === 'directional') l = new THREE.DirectionalLight(color, spec.intensity);
  else if (spec.kind === 'point') l = new THREE.PointLight(color, spec.intensity, spec.distance ?? 0);
  else l = new THREE.SpotLight(color, spec.intensity, spec.distance ?? 0, spec.outer, 1 - spec.inner / spec.outer);
  l.userData.frames = spec.frames;
  return l;
}
function placeLight(l, key, scene) {
  const f = l.userData.frames?.[key] ?? l.userData.frames?.static;
  if (!f) return;
  l.position.fromArray(f.position);
  if (l.target) {
    if (!l.target.parent) scene.add(l.target);
    l.target.position.fromArray(f.position).add(new THREE.Vector3().fromArray(f.direction || [0, 0, -1]));
    l.target.updateMatrixWorld(true);
  }
  l.updateMatrixWorld(true);
}
const maxChannel = (px) => {
  let m = 0;
  for (let i = 0; i < px.length; i += 4) if (px[i + 3] > 0) m = Math.max(m, px[i], px[i + 1], px[i + 2]);
  return m;
};
const srgbToLinear = (c) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));

// Every texture of a loaded file sampled from its base level alone: no mip
// chain, so no minification filter either side. A KTX2 original carries the mip
// levels its encoder made, where a PNG reading gets the ones the GPU builds by
// box filtering, so at 256 px the two sample different levels of identical
// pixels (Khronos AnisotropyBarnLamp's 2048 px base colour: 9.54 % of pixels
// apart past the albedo tolerance of 20, 2026-11-08). With `level0` the
// native check's KTX2 pass compares the content alone.
function baseLevelOnly(root) {
  const seen = new Set();
  root.traverse((o) => {
    for (const m of Array.isArray(o.material) ? o.material : [o.material]) {
      for (const [slot, t] of Object.entries(m || {})) {
        if (!t || !t.isTexture || seen.has(t)) continue;
        seen.add(t);
        t.minFilter = THREE.LinearFilter;
        t.generateMipmaps = false;
        // KTX2Loader tags a texture with its file's DFD colour space, and
        // GLTFLoader.assignTexture overrides it for the four slots glTF
        // defines as sRGB alone (:3576, :3668, :1122, :1323), so a map in any
        // other slot keeps the DFD's tag. The samples' tags are often wrong
        // (ORM maps tagged sRGB: the design's research), where litegen writes
        // the stored values and glTF says which slot is a colour. So a KTX2
        // texture takes the colour space of its slot, set before its first
        // upload, as it does for a PNG.
        if (t.userData?.mimeType === 'image/ktx2') t.colorSpace = KTX2_SRGB_SLOTS.has(slot) ? THREE.SRGBColorSpace : THREE.NoColorSpace;
      }
    }
  });
}
const KTX2_SRGB_SLOTS = new Set(['map', 'emissiveMap', 'sheenColorMap', 'specularColorMap']);

// `viewAspect`: draws the file's perspective camera at that aspect (the
// expected picture of a camera with no aspect ratio at the 16:9 litegen
// writes for it, UNSET_ASPECT, page/camera-view.js).
// `playClip`, `alsoRigidOf` (mesh-gaps item 6, several clips in USD): see where
// `playing` is chosen below.
async function renderFile({ url, format, files, camera, modes, times, clipName, autoFrame, presentUvSetsOnly, alphaStats, external, cameraView, cameraName, blind, viewAspect, level0, playClip, alsoRigidOf }) {
  const { root, clips, cameras: loadedCameras, associations, ranks, json } = await load(url, format, files);
  if (level0) baseLevelOnly(root);
  const nodeName = (o) => lightNodeName(o, associations, ranks);
  const texturesLoaded = await texturesReady(root);
  // Read off the materials as loaded, before any mode replaces them: whether
  // every base colour is 0 or 1 in each channel, and the highest uv set a
  // base-colour texture samples (see usdAlbedoJudgeable).
  const loaded = meshes(root).flatMap((m) => (Array.isArray(m.material) ? m.material : [m.material])).filter(Boolean);
  const unitColors = loaded.every((m) => !m.color || [m.color.r, m.color.g, m.color.b].every((c) => c === 0 || c === 1));
  const unitEmissive = loaded.every((m) => !m.emissive || unit(m.emissive));
  const mapSet = Math.max(0, ...loaded.filter((m) => m.map).map((m) => m.map.channel || 0));
  // Which material layers the loader built (MeshPhysicalMaterial's
  // properties, each at other than its default), and whether a clearcoat is
  // textured: the `lit` mode is asked for where the expected file has a
  // layer, and three.js's USDLoader follows no clearcoat texture (verify.mjs).
  const layers = [...new Set(loaded.flatMap(materialLayers))];
  const coatTextured = loaded.some((m) => m.clearcoatMap || m.clearcoatRoughnessMap || m.clearcoatNormalMap);
  // An expected file without a layer draws no lit mode: nothing asks for its
  // lit pictures (verify.mjs, litPlan skips such a case outright).
  if (presentUvSetsOnly && !layers.length) modes = modes.filter((m) => !LIT_MODES[m]);
  // An expected picture draws a per-set uv mode only for a set some mesh
  // has: the cases ask for no other (verify.mjs), and a set no mesh has would
  // only draw magenta.
  if (presentUvSetsOnly) {
    const sets = uvSetCount(root);
    modes = modes.filter((m) => UV_SET_MODES.indexOf(m) < 1 || UV_SET_MODES.indexOf(m) < sets);
  }
  // The points and lines the loader built (WIRE), and the wire pictures only
  // for what the expected file holds.
  const wire = wireCounts(root);
  if (presentUvSetsOnly) {
    modes = modes.filter((m) => (m !== 'wire_cover_points' || wire.points > 0) && (m !== 'wire_cover_lines' || wire.lines > 0));
  }
  // The file's own lights and first camera (CAMLIGHT), or `external`'s in
  // their place. An expected file draws the camera modes only when it has a
  // camera, the light modes only when it has lights, and `lights_each` as
  // one `light:<node>` picture per light.
  const fileLights = [];
  root.traverse((o) => {
    if (o.isLight) fileLights.push(o);
  });
  // The document's cameras the scene places, in the document's order, then
  // every other camera in the scene, in scene order: a camera two nodes share
  // is cloned onto each (GLTFLoader, _getNodeRef), and the clones are the
  // placed ones. The first is the file's first camera; `cameraName` (a name,
  // or a list of the names a reader may give one node) picks one by its
  // node's name instead, as every case and our side of a native pair are
  // drawn through the camera named like the reference's first.
  const inScene = (o) => {
    for (let p = o; p; p = p.parent) if (p === root) return true;
    return false;
  };
  const fileCameras = (loadedCameras ?? []).filter(inScene);
  root.traverse((o) => {
    if (o.isCamera && !fileCameras.includes(o)) fileCameras.push(o);
  });
  const wanted = cameraName === undefined ? null : [].concat(cameraName);
  const chosen = wanted ? fileCameras.find((c) => wanted.includes(nodeName(c))) : fileCameras[0];
  const fileCamera = external?.camera ? externalCamera(external.camera) : (chosen ?? null);
  // Whether the camera is a perspective one whose glTF has no aspectRatio
  // (GLTFLoader gives such a camera aspect 1: GLTFLoader.js loadCamera,
  // `params.aspectRatio || 1`): read off the node's own definition.
  let cameraAspectUnset = false;
  if (chosen && !external?.camera && json) {
    for (let o = chosen; o && !cameraAspectUnset; o = o.parent) {
      const i = associations?.get(o)?.nodes;
      if (i === undefined) continue;
      const def = json.cameras?.[json.nodes?.[i]?.camera];
      cameraAspectUnset = def?.type === 'perspective' && def.perspective?.aspectRatio === undefined;
      break;
    }
  }
  if (viewAspect && fileCamera?.isPerspectiveCamera) {
    fileCamera.aspect = viewAspect;
    fileCamera.updateProjectionMatrix();
  }
  const extLights = (external?.lights || []).map(externalLight);
  if (presentUvSetsOnly) {
    modes = modes
      .filter((m) => (!CAMERA_MODES[m] || fileCamera) && ((m !== 'lit_lights' && m !== 'lights_each') || fileLights.length))
      .flatMap((m) => (m === 'lights_each' ? fileLights.flatMap((l) => [`light:${nodeName(l)}`, `lightsk:${nodeName(l)}`]) : [m]));
  }
  const view = fileCamera ? (cameraView ?? cameraViewOf(fileCamera)) : null;
  // Taken at rest, before any clip moves a bone.
  const singularBone = meshes(root).some(
    (m) => m.isSkinnedMesh && m.skeleton.bones.some((b) => b.scale.x === 0 || b.scale.y === 0 || b.scale.z === 0),
  );
  const scene = new THREE.Scene();
  scene.add(root);
  if (autoFrame) camera = frameCamera(root, blind === 'wire' ? drawables(root).filter(isWire) : []);
  const cam = new THREE.PerspectiveCamera(camera.fov, 1, camera.near, camera.far);
  cam.position.fromArray(camera.position);
  cam.lookAt(new THREE.Vector3().fromArray(camera.target));
  cam.updateMatrixWorld(true);

  // Default morph weights, read before any clip moves them (GLTFLoader
  // starts each mesh at its `weights`): see FBX_STATIC_MORPH below.
  const morphDefaults = meshes(root).some((m) => (m.morphTargetInfluences || []).some((w) => w !== 0));
  let mixer = null;
  let clip = null;
  const actions = [];
  if (times && times.length && clips.length) {
    clip = clips.find((c) => c.name === clipName) || clips[0];
    mixer = new THREE.AnimationMixer(root);
    // USD has one timeline, which USDLoader splits into a clip per kind of
    // data: a SkelAnimation's ("Anim") and the xformOps'
    // ("TransformAnimation"). Played alone, the first leaves every animated
    // xform at rest. So for USD, all of them play.
    // A multi-clip USD file (an `Animation` variant set: item 6) lists a clip per
    // SkelAnimation, which all playing at once would fight over the joints:
    // `playClip` names the one that plays, with the TransformAnimation, the
    // first clip's rigid motion, which sits outside the variants and so plays
    // whichever variant is selected.
    let playing = USD_ANY[format] ? (playClip ? clips.filter((c) => c.name === playClip || c.name === 'TransformAnimation') : clips) : [clip];
    // The expected picture of a clip after the first (`alsoRigidOf`, the first
    // clip's name): that clip's skeletal channels beside the first one's rigid
    // channels (the tracks of nodes that are no bone), which is what a USD
    // file plays under every variant: the first clip's rigid xformOps sit
    // outside the variants and the other clips' rigid motion is dropped.
    if (alsoRigidOf) {
      const first = clips.find((c) => c.name === alsoRigidOf);
      if (first && first !== clip) {
        const bones = new Set(meshes(root).flatMap((m) => (m.isSkinnedMesh ? m.skeleton.bones.map((b) => THREE.PropertyBinding.sanitizeNodeName(b.name)) : [])));
        const onBone = (t) => bones.has(THREE.PropertyBinding.parseTrackName(t.name).nodeName);
        const rigid = first.tracks.filter((t) => !onBone(t));
        playing = [new THREE.AnimationClip(clip.name, -1, clip.tracks.filter(onBone)), ...(rigid.length ? [new THREE.AnimationClip(`${first.name}:rigid`, -1, rigid)] : [])];
      }
    }
    for (const c of playing) {
      const action = mixer.clipAction(c);
      // LoopOnce + clamp, or the final sample wraps to frame 0.
      action.setLoop(THREE.LoopOnce, 1);
      action.clampWhenFinished = true;
      action.play();
      actions.push(action);
    }
  }
  for (const m of drawables(root)) m.frustumCulled = false;
  for (const l of extLights) scene.add(l);
  // A `light:<node>` picture's exposure, fixed by its rest pose
  // (LinearToneMapping, a pure scale before the sRGB encoding), by node; a
  // `lightsk:<node>` picture's as `sk:<node>`.
  const lightExposure = new Map();
  // How many pixels each light lights in a picture of it alone, per time
  // key, with its range as the file has it (as `lit_lights` draws it): where
  // a light lights nothing the camera sees, a picture of every light cannot
  // judge it (verify.mjs, camlightJudged).
  const lightLit = {};
  // A light drawn as one reader draws it (the light modes above), and how to
  // put it back: no range for both; for SceneKit no falloff, and its colour
  // read as sRGB.
  const asReaderDraws = (l, sceneKit) => {
    const saved = { distance: l.distance, decay: l.decay, color: l.color.clone(), intensity: l.intensity };
    if (l.isPointLight || l.isSpotLight) {
      l.distance = 0;
      if (sceneKit) l.decay = 0;
    }
    if (sceneKit) {
      const c = [l.color.r, l.color.g, l.color.b].map((v) => srgbToLinear(Math.min(1, v * saved.intensity)));
      l.color.setRGB(c[0], c[1], c[2], THREE.LinearSRGBColorSpace);
      l.intensity = 1;
    }
    return () => {
      if (saved.distance !== undefined) l.distance = saved.distance;
      if (saved.decay !== undefined) l.decay = saved.decay;
      l.color.copy(saved.color);
      l.intensity = saved.intensity;
    };
  };
  const shot = (exposure) => {
    renderer.toneMapping = THREE.LinearToneMapping;
    renderer.toneMappingExposure = exposure;
    renderer.render(scene, cam);
    renderer.toneMapping = THREE.NoToneMapping;
    renderer.toneMappingExposure = 1;
    return capture();
  };
  const exposed = (node) => {
    if (lightExposure.has(node)) return shot(lightExposure.get(node));
    let e = 1;
    let px = shot(e);
    // A light too faint to show at exposure 1 is brightened 16× at a time
    // until a pixel shows, at most 16^10 (Blender's FBX export of `staged`
    // writes its 3 lux sun as 0.0044, and SceneKit's reading of that, decoded
    // from sRGB, drew black); one that still shows nothing lights nothing.
    for (let i = 0; i < 10 && maxChannel(px) === 0; i++) px = shot((e *= 16));
    if (maxChannel(px) === 0) px = shot((e = 1));
    for (let i = 0; i < 16; i++) {
      const m = maxChannel(px);
      if (m === 0) break;
      if (m >= 250) {
        e /= 4;
        px = shot(e);
        continue;
      }
      e *= srgbToLinear(LIGHT_TARGET / 255) / srgbToLinear(m / 255);
      px = shot(e);
      break;
    }
    lightExposure.set(node, e);
    return px;
  };
  // Only the lit modes light anything: the others' materials are unlit. Each
  // light shines in every mode, as loaded (item 4's `lit` included), but a
  // `light:<node>` picture, where one shines alone, and when `external`
  // lights stand in for the file's.
  const draw = (mode, key) => {
    applyMode(root, mode, format, blind);
    // `lit_coat` draws the USDLoader-judgeable meshes alone (both sides).
    for (const m of drawables(root)) m.visible = shown(m, mode, blind);
    const alone = isLightMode(mode) ? lightModeNode(mode) : null;
    for (const l of fileLights) l.visible = !extLights.length && (alone === null || nodeName(l) === alone);
    for (const l of extLights) placeLight(l, key, scene);
    scene.environment = LIT_MODES[mode]?.environment || null;
    scene.environmentIntensity = LIT_MODES[mode]?.exposure ?? 1;
    root.updateMatrixWorld(true);
    if (CAMERA_MODES[mode]) {
      if (!fileCamera) return null;
      if (fileCamera.userData.frames) placeCamera(fileCamera, key);
      renderer.setSize(view.width, view.height);
      renderer.render(scene, fileCamera);
      const px = capture(view.width, view.height);
      renderer.setSize(SIZE, SIZE);
      return px;
    }
    if (alone !== null) {
      const sceneKit = isSceneKitLightMode(mode);
      const kept = fileLights.filter((l) => l.visible);
      const restore = kept.map((l) => asReaderDraws(l, sceneKit));
      const px = exposed(sceneKit ? `sk:${alone}` : alone);
      for (const r of restore) r();
      if (!sceneKit) {
        // Counted with the range the file gives the light, as the whole-
        // scene picture draws it, at the same exposure.
        const ranged = kept.some((l) => (l.isPointLight || l.isSpotLight) && l.distance > 0);
        const counted = ranged ? shot(lightExposure.get(alone)) : px;
        let n = 0;
        for (let i = 0; i < counted.length; i += 4) if (counted[i + 3] > 0 && Math.max(counted[i], counted[i + 1], counted[i + 2]) >= LIGHT_LIT) n++;
        (lightLit[alone] ??= {})[key] = n;
      }
      return px;
    }
    if (mode === 'lit_lights') return shot(LIT_LIGHTS_EXPOSURE);
    renderer.render(scene, cam);
    return capture();
  };
  // The default pose of every mode first: once the mixer has moved the
  // skeleton there is no cheap way back to it.
  const out = {};
  for (const mode of modes) out[mode] = { static: draw(mode, 'static') };
  if (mixer) {
    for (const t of times) {
      // An action that finished (LoopOnce, clamped) at an earlier time stays
      // paused, and `setTime` would then hold it at time 0 instead of its last
      // pose: a clip shorter than the times asked for (a USD file's first clip
      // beside a longer one, item 6) must be evaluated afresh at each time.
      for (const a of actions) {
        a.paused = false;
        a.enabled = true;
      }
      mixer.setTime(t);
      for (const mode of modes) out[mode][String(t)] = draw(mode, String(t));
    }
  }
  const stats = { meshes: meshes(root).length, clips: clips.map((c) => `${c.name}:${c.duration.toFixed(3)}s`), animated: !!mixer, texturesLoaded, morphDefaults };
  let triangles = 0;
  for (const m of meshes(root)) {
    const g = m.geometry;
    triangles += g.index ? g.index.count / 3 : g.attributes.position.count / 3;
  }
  stats.triangles = triangles;
  // How many Points and Line objects (WIRE) the loader built with a vertex to
  // draw: verify.mjs plans the wire judges from the expected file's.
  stats.points = wire.points;
  stats.lines = wire.lines;
  stats.pointVertices = wire.vertices;
  // Whether the file itself carries normals on every mesh, and UVs on any.
  // Loaders disagree about inventing normals (OBJLoader computes them when a
  // file has none; GLTFLoader does not), so the normals mode is compared only
  // when the EXPECTED file has them everywhere. No loader invents UVs, and a
  // mesh without them draws magenta in the uv mode on both sides, so that
  // mode compares whenever any mesh has them: requiring every mesh skipped
  // it on the textured fixture, whose wedge has none (2026-10-12 review #31).
  stats.hasNormals = meshes(root).every((m) => !!m.geometry.attributes.normal);
  stats.hasUv = meshes(root).some((m) => !!m.geometry.attributes.uv);
  // How many uv sets the file holds (up to the highest any mesh has): the
  // per-set modes verify.mjs asks a case for.
  stats.uvSets = uvSetCount(root);
  stats.vertexColors = meshes(root).some((m) => !!m.geometry.attributes.color);
  stats.unitColors = unitColors;
  stats.unitEmissive = unitEmissive;
  stats.mapSet = mapSet;
  stats.layers = layers;
  stats.coatTextured = coatTextured;
  // The textures KTX2Loader built (a KHR_texture_basisu image: GLTFLoader tags
  // each with its source's mime type), and the GPU format each was transcoded
  // to on this machine (ktx2Loader above), for the report.
  const ktx2 = new Set(loaded.flatMap((m) => Object.values(m).filter((v) => v?.isTexture && v.userData?.mimeType === 'image/ktx2')));
  if (ktx2.size) {
    const formats = {};
    for (const t of ktx2) formats[ktx2FormatName(t.format)] = (formats[ktx2FormatName(t.format)] || 0) + 1;
    stats.ktx2 = { textures: ktx2.size, formats, mode: ktx2Mode, support: ktx2Active().workerConfig };
  }
  // Alpha the expected albedo and coverage pictures do not draw (Ruling
  // A37): a translucent or cut-out material (BLEND, MASK, an opacity below 1),
  // a base-colour texture with an alpha below 1 somewhere, and vertex colours
  // with an alpha below 1 (a PLY's baked translucency, read with no material:
  // glTF draws it opaque). All are drawn as one opaque colour here
  // (albedoMaterial), where SceneKit and usdrecord draw them blended, so
  // verify.mjs skips those two oracles' albedo and silhouette where the
  // expected file has any. Read for expected files alone (`alphaStats`): it
  // scans each base-colour image once.
  if (alphaStats) {
    stats.translucent = loaded.some((m) => m.transparent || m.alphaTest > 0 || m.opacity < 1);
    stats.textureAlpha = loaded.some((m) => m.map && imageHasAlpha(m.map.image));
    stats.vertexAlpha = meshes(root).some((m) => {
      const c = m.geometry.attributes.color;
      if (!c || c.itemSize < 4) return false;
      for (let i = 0; i < c.count; i++) if (c.getW(i) < 1) return true;
      return false;
    });
    // The names on each mesh's chain of ancestors (the root excluded), and
    // whether its material is unlit as loaded (GLTFLoader builds a
    // MeshBasicMaterial for KHR_materials_unlit): the unlit check tells the
    // unlit meshes from the lit control by these, in whatever file it draws
    // (usd-variants.mjs, groupNames).
    stats.nodeChains = meshes(root).map((m) => {
      const names = [];
      for (let o = m; o && o !== root; o = o.parent) if (o.name) names.push(o.name);
      const held = m.userData.original ?? m.material;
      return { names, unlit: (Array.isArray(held) ? held : [held]).every((x) => x && x.isMeshBasicMaterial) };
    });
  }
  // How many meshes `lit_coat` draws, and the layers those meshes carry: the
  // USD targets' lit judge has something to judge only where a drawn mesh
  // carries a layer it can see (verify.mjs, litPlan).
  const coatMeshes = meshes(root).filter(usdLitJudgeable);
  stats.litCoatMeshes = coatMeshes.length;
  stats.litCoatLayers = [
    ...new Set(coatMeshes.flatMap((m) => (Array.isArray(m.userData.original ?? m.material) ? m.userData.original ?? m.material : [m.userData.original ?? m.material]).flatMap(materialLayers))),
  ];
  stats.skinned = meshes(root).some((m) => m.isSkinnedMesh);
  stats.singularBone = singularBone;
  stats.morphed = meshes(root).some((m) => (m.geometry.morphAttributes.position || []).length > 0);
  // Morph targets that move normals as well as positions (see
  // FBX_MORPH_NORMALS).
  stats.morphNormals = meshes(root).some((m) => (m.geometry.morphAttributes.normal || []).length > 0);
  // The lights and cameras the loader built (CAMLIGHT): verify.mjs plans the
  // camera and light judges from the expected file's.
  stats.lights = fileLights.map((l) => ({ name: nodeName(l), kind: lightKind(l) }));
  // The chosen camera first: the one every judge draws through.
  stats.cameras = [...(chosen ? [chosen] : []), ...fileCameras.filter((c) => c !== chosen)].map((c) => ({ name: nodeName(c), kind: c.isOrthographicCamera ? 'orthographic' : 'perspective' }));
  stats.cameraView = view;
  stats.cameraAspectUnset = cameraAspectUnset;
  stats.lightExposure = Object.fromEntries(lightExposure);
  stats.lightLit = lightLit;
  scene.remove(root);
  for (const l of extLights) scene.remove(l, ...(l.target ? [l.target] : []));
  return { frames: out, stats, camera };
}

// A picture's width and height: the camera modes' are the file camera's view.
const sizeOf = (mode, stats) => (CAMERA_MODES[mode] && stats.cameraView ? [stats.cameraView.width, stats.cameraView.height] : [SIZE, SIZE]);

window.renderExpected = async (job) => {
  const r = await renderFile(job);
  expectedInfo.set(job.key, r.stats);
  const images = {};
  for (const [mode, byT] of Object.entries(r.frames)) {
    images[mode] = {};
    for (const [t, px] of Object.entries(byT)) {
      expectedCache.set(`${job.key}|${mode}|${t}`, px);
      images[mode][t] = toPng(px, ...sizeOf(mode, r.stats));
    }
  }
  // An expected camera with no aspect ratio is also drawn at the 16:9 that
  // litegen writes for it to the formats that always store one (UNSET_ASPECT):
  // `<key>#aspect`, in `camera_cover` alone, the picture usdrecord's is
  // compared with (verify.mjs, the usdrecord camera judge). It stays in the
  // cache alone.
  if (r.stats.cameraAspectUnset && job.modes.includes('camera_cover')) {
    const v = await renderFile({ ...job, modes: ['camera_cover'], viewAspect: UNSET_ASPECT });
    expectedInfo.set(`${job.key}#aspect`, v.stats);
    for (const [t, px] of Object.entries(v.frames.camera_cover || {})) expectedCache.set(`${job.key}#aspect|camera_cover|${t}`, px);
  }
  // An expected file with points or lines is also drawn as a reader that
  // cannot read all of them would draw it: with its lines' vertex colours left
  // out (`<key>#linecolors`), without its lines (`<key>#lines`) and without its
  // points and lines (`<key>#wire`), in every mode but the wire pictures: the
  // pictures those readers are compared with (WIRE, verify.mjs wirePlan). They
  // stay in the cache alone.
  if (r.stats.points + r.stats.lines > 0) {
    for (const blind of WIRE_BLINDS) {
      if (blind !== 'wire' && !r.stats.lines) continue;
      const v = await renderFile({ ...job, blind, modes: job.modes.filter((m) => !m.startsWith('wire_')) });
      expectedInfo.set(`${job.key}#${blind}`, r.stats);
      for (const [mode, byT] of Object.entries(v.frames)) {
        for (const [t, px] of Object.entries(byT)) expectedCache.set(`${job.key}#${blind}|${mode}|${t}`, px);
      }
    }
  }
  return { images, stats: r.stats };
};

// A documented limitation of an oracle loader, not a tolerance: three.js's
// FBXLoader reads a blend-shape channel's static DeformPercent into
// `initialWeight` (FBXLoader.js r183, parseMorphTargets: `initialWeight:
// morphTargetNode.DeformPercent`) and never uses it — the field has no other
// reference in the loader — so every FBX mesh starts with all morph
// influences at 0 whatever the file says. A file whose default weights are
// not zero therefore cannot match its expected picture in the DEFAULT pose
// under three.js, however it is written; the animated frames, where the clip
// sets every influence, are compared as usual. Our own FBX reader honours
// DeformPercent (the fbx -> glb cases prove it in the default pose).
const FBX_STATIC_MORPH =
  'three.js FBXLoader never applies DeformPercent as the initial morph weight (parseMorphTargets stores it as an unused initialWeight)';

// Likewise: FBXLoader builds blend shapes from positions only
// (FBXLoader.js r183, genMorphGeometry: "TODO: add morph normal support";
// generateMorphGeometry: `// parentGeo.morphAttributes.normal = []; // not
// implemented`), so wherever a clip drives a target that moves normals, its
// lighting keeps the base normals, however the file is written. The geom
// mode (normals derived from the morphed positions) still checks those
// frames, and our reader brings shape normals back (fbx tests,
// morph_targets_keep_names_defaults_and_normals).
const FBX_MORPH_NORMALS =
  'three.js FBXLoader does not morph normals (genMorphGeometry: "TODO: add morph normal support"); positions are checked by geom';

// The camera `renderPair` would frame a file with, for renderers outside the
// page (usdrecord) to use the same view.
window.frameOf = async (url, format) => {
  const { root } = await load(url, format, []);
  return frameCamera(root);
};

// What the expected picture of `key` holds (skins, morph targets, ...).
window.expectedStats = (key) => expectedInfo.get(key) || null;

// Whether a USD file of the expected scene `key` can be judged in albedo
// (usdAlbedoJudgeable): verify.mjs asks before SceneKit's USD albedo.
window.usdAlbedoJudgeable = (key) => usdAlbedoJudgeable(expectedInfo.get(key) || {});

window.renderCase = async (job) => {
  let r;
  const info = expectedInfo.get(job.expectedKey) || {};
  try {
    // A case's camera pictures are drawn at its expected picture's size.
    r = await renderFile({ cameraView: info.cameraView ?? undefined, cameraName: info.cameras?.[0]?.name, ...job });
  } catch (e) {
    return { error: String(e && e.stack ? e.stack.split('\n').slice(0, 3).join(' | ') : e) };
  }
  const results = {};
  const keys = ['static', ...(job.times || []).map(String)];
  // A USDZ whose root layer is a crate (`--usd-variants`, usdz-crate) is read
  // as the crate it holds, so the text-root limitations do not apply to it.
  const limitFormat = job.crateRoot ? 'usdc' : job.format;
  for (const [mode, byT] of Object.entries(r.frames)) {
    results[mode] = {};
    if (CAMERA_MODES[mode] && !byT.static) {
      results[mode].static = { error: 'the file has no camera to draw through' };
      continue;
    }
    const [w, h] = sizeOf(mode, r.stats);
    // A limitation that holds at every time skips the mode outright. A
    // skipped lit picture stays in the report (the reader is not a judge of
    // it, but what it drew is still evidence).
    const always = usdLimitation(limitFormat, mode, 'static', info);
    if (always && usdLimitation(limitFormat, mode, 'animated', info)) {
      results[mode].static = { skipped: always, ...(mode === 'lit' ? { image: toPng(byT.static) } : {}) };
      continue;
    }
    if ((mode === 'normals' && info.hasNormals === false) || (mode === 'uv' && info.hasUv === false)) {
      results[mode].static = { skipped: `the source has no ${mode === 'uv' ? 'UVs' : 'normals'}` };
      continue;
    }
    for (const t of keys) {
      if (t === 'static' && job.format === 'fbx' && info.morphDefaults) {
        results[mode][t] = { skipped: FBX_STATIC_MORPH };
        continue;
      }
      if (t !== 'static' && job.format === 'fbx' && mode === 'normals' && info.morphNormals) {
        results[mode][t] = { skipped: FBX_MORPH_NORMALS };
        continue;
      }
      // A file that lost its animation shows its default pose at every time,
      // and is compared as such: the frames that should have moved fail.
      const px = byT[t] ?? byT.static;
      const limit = usdLimitation(limitFormat, mode, t, info);
      if (limit) {
        results[mode][t] = { skipped: limit, ...(mode === 'lit' ? { image: toPng(px) } : {}) };
        continue;
      }
      // An unanimated expected picture holds its default pose at every time.
      const want =
        expectedCache.get(`${job.expectedKey}|${mode}|${t}`) ?? expectedCache.get(`${job.expectedKey}|${mode}|static`);
      if (!want) {
        results[mode][t] = { error: 'no expected picture' };
        continue;
      }
      if (want.length !== px.length) {
        results[mode][t] = { error: `drawn at ${w}x${h}, not at the expected picture's size` };
        continue;
      }
      const cmp = compare(want, px, LIT_MODES[mode] || mode === 'lit_lights' ? LIT_TOLERANCE : 20);
      results[mode][t] = {
        ratio: cmp.ratio,
        differ: cmp.differ,
        covered: cmp.covered,
        maxDelta: cmp.maxDelta,
        image: toPng(px, w, h),
        diff: cmp.differ ? toPng(cmp.diff, w, h) : null,
      };
    }
  }
  return { results, stats: r.stats };
};

// Which of an expected file's layers the lit judges can see, measured
// (verify.mjs asks for every expected file that keeps a layer): each
// KHR_materials_* extension is deleted from every material in turn (and from
// extensionsUsed), the variant GLB is rendered through GLTFLoader from a
// blob, and compared with the expected picture at LIT_TOLERANCE in each mode
// asked for. A layer is seen in a mode when a pixel moves past the
// tolerance. The reviewer's probe of 2026-11-05 built in: a layer that moves
// nothing in this scene at this exposure (dispersion; an ior of 1.52 on an
// uncoated glass) makes a measured skip, not a judged case (verify.mjs,
// litPlan).
window.layerVisibility = async (job) => {
  const buf = new Uint8Array(await (await fetch(job.url)).arrayBuffer());
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  const jsonLength = view.getUint32(12, true);
  const json = JSON.parse(new TextDecoder().decode(buf.subarray(20, 20 + jsonLength)));
  const rest = buf.subarray(20 + jsonLength);
  const exts = [...new Set((json.materials || []).flatMap((m) => Object.keys(m.extensions || {})))].filter((e) => e.startsWith('KHR_materials_'));
  const out = {};
  for (const ext of exts) {
    const j = JSON.parse(JSON.stringify(json));
    for (const m of j.materials) if (m.extensions) delete m.extensions[ext];
    j.extensionsUsed = (j.extensionsUsed || []).filter((e) => e !== ext);
    const encoded = new TextEncoder().encode(JSON.stringify(j));
    const text = new Uint8Array(encoded.length + ((4 - (encoded.length % 4)) % 4));
    text.set(encoded);
    text.fill(0x20, encoded.length);
    const head = new Uint8Array(20);
    const hv = new DataView(head.buffer);
    hv.setUint32(0, 0x46546c67, true); // 'glTF'
    hv.setUint32(4, 2, true);
    hv.setUint32(8, 20 + text.length + rest.length, true);
    hv.setUint32(12, text.length, true);
    hv.setUint32(16, 0x4e4f534a, true); // 'JSON'
    const url = URL.createObjectURL(new Blob([head, text, rest], { type: 'model/gltf-binary' }));
    let r;
    try {
      r = await renderFile({ url, format: 'glb', files: [], camera: job.camera, modes: job.modes, times: [] });
    } finally {
      URL.revokeObjectURL(url);
    }
    const layer = ext.replace('KHR_materials_', '');
    out[layer] = {};
    for (const mode of job.modes) {
      const want = expectedCache.get(`${job.key}|${mode}|static`);
      const px = r.frames[mode]?.static;
      if (!want || !px) continue;
      const cmp = compare(want, px, LIT_TOLERANCE);
      out[layer][mode] = { ratio: cmp.ratio, differ: cmp.differ, covered: cmp.covered, maxDelta: cmp.maxDelta, seen: cmp.differ > 0 };
    }
  }
  return out;
};

// Compare a picture rendered OUTSIDE the browser (usdrecord, Pixar's Hydra
// Storm renderer) with an expected picture.
//
//  - silhouette: alpha coverage only, against the expected `cover` picture
//    (both sides drawn, as Storm draws them). Proves positions, transforms,
//    skinning, morph targets and animation timing.
//  - albedo: colour on pixels covered in both and not on either silhouette's
//    edge; a pixel matches when any pixel of the other image's 3x3
//    neighbourhood is within tolerance, which absorbs the two renderers'
//    different texture filtering and edge rasterisation while a wrong colour
//    space, a wrong texture placement or a missing vertex colour still fails
//    nearly every pixel (measured negative controls: 98% and 82%).
window.compareExternal = async (job) => {
  const img = new Image();
  img.src = job.url;
  await img.decode();
  // usdrecord --camera draws at the camera's own aspect (`job.width`,
  // `job.height`); the size check, and the one row it excuses, are
  // externalPictureSize's (page/camera-view.js).
  const [W, H] = [job.width ?? SIZE, job.height ?? SIZE];
  let rescaled = null;
  if (job.width) {
    const size = externalPictureSize({ width: img.naturalWidth, height: img.naturalHeight }, { width: W, height: H });
    if (size.error) return { error: size.error };
    rescaled = size.rescaled ?? null;
  }
  const c = document.createElement('canvas');
  c.width = W;
  c.height = H;
  const ctx = c.getContext('2d');
  ctx.drawImage(img, 0, 0, W, H);
  const px = ctx.getImageData(0, 0, W, H).data;
  const want =
    expectedCache.get(`${job.expectedKey}|${job.mode}|${job.t}`) ?? expectedCache.get(`${job.expectedKey}|${job.mode}|static`);
  if (!want) return { error: 'no expected picture' };
  const diff = new Uint8ClampedArray(px.length);
  const covered = (a, i) => a[i + 3] > 0;
  let total = 0;
  let differ = 0;
  if (job.kind === 'silhouette') {
    // Coverage differs only where the other image has no matching coverage
    // within one pixel: the renderers' edge rules differ by a pixel, which on
    // a thin limb is several percent of it, while a misposed part moves
    // its silhouette by many.
    const near = (img, x, y, want1) => {
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const xx = x + dx;
          const yy = y + dy;
          if (xx < 0 || yy < 0 || xx >= W || yy >= H) continue;
          if (covered(img, (yy * W + xx) * 4) === want1) return true;
        }
      }
      return false;
    };
    for (let y = 0; y < H; y++) {
      for (let x = 0; x < W; x++) {
        const i = (y * W + x) * 4;
        const a = covered(want, i);
        const b = covered(px, i);
        if (!a && !b) continue;
        total++;
        if (a !== b && !(a ? near(px, x, y, true) : near(want, x, y, true))) {
          differ++;
          diff.set([255, 0, 60, 255], i);
        } else {
          diff.set([150, 150, 150, 255], i);
        }
      }
    }
  } else {
    const tol = 20;
    const interior = (a, x, y) =>
      [[1, 0], [-1, 0], [0, 1], [0, -1]].every(([dx, dy]) => {
        const xx = x + dx;
        const yy = y + dy;
        return xx >= 0 && yy >= 0 && xx < W && yy < H && covered(a, (yy * W + xx) * 4);
      });
    for (let y = 0; y < H; y++) {
      for (let x = 0; x < W; x++) {
        const i = (y * W + x) * 4;
        if (!covered(want, i) || !covered(px, i) || !interior(want, x, y) || !interior(px, x, y)) continue;
        total++;
        let ok = false;
        for (let dy = -1; dy <= 1 && !ok; dy++) {
          for (let dx = -1; dx <= 1 && !ok; dx++) {
            const xx = x + dx;
            const yy = y + dy;
            if (xx < 0 || yy < 0 || xx >= W || yy >= H) continue;
            const j = (yy * W + xx) * 4;
            if (!covered(px, j)) continue;
            const d = Math.max(Math.abs(want[i] - px[j]), Math.abs(want[i + 1] - px[j + 1]), Math.abs(want[i + 2] - px[j + 2]));
            if (d <= tol) ok = true;
          }
        }
        if (!ok) {
          differ++;
          diff.set([255, 0, 60, 255], i);
        } else {
          const g = 70 + (want[i] + want[i + 1] + want[i + 2]) / 12;
          diff.set([g, g, g, 255], i);
        }
      }
    }
  }
  return { ratio: total ? differ / total : 0, differ, covered: total, image: toPng(new Uint8ClampedArray(px), W, H), diff: differ ? toPng(diff, W, H) : null, ...(rescaled ? { rescaled } : {}) };
};

// The wire judge (WIRE): a picture of a file's points and lines alone, drawn
// OUTSIDE the browser (usdrecord, SceneKit), against an expected
// `wire_cover_points` or `wire_cover_lines` picture, on the bounding box and
// the centroid of the covered pixels. A point or a line cannot be compared
// pixel for pixel across renderers: Storm draws a point at its world-space
// `widths` and SceneKit its own size, where the expected picture's are
// POINT_SIZE px and 1 px. So each edge of the box and each coordinate of the
// centroid must lie within 1 % of the frame (verify.mjs, WIRE_LIMIT) of the
// expected one. A picture drawn at `job.width` px (verify.mjs, WIRE_USD_WIDTH:
// Storm covers a pixel by quarters, so a point under a pixel wide can fall
// between its samples) is measured in the expected picture's pixels, a
// covered pixel by its centre. `ratio` is the largest of those six distances
// over the frame (SIZE); a picture that draws nothing where one is expected is
// 1, and two empty pictures (`empty`) judge nothing. For points, where the
// expected picture shows every one of the file's `job.blobs` points apart (its
// blobs are as many), the centroid is the mean of the points' own centres,
// each connected blob of covered pixels counting once: a point's footprint
// differs by renderer (4 px squares here, a pixel or two in Storm, by how the
// quarter-sample coverage falls), and a centroid of the pixels would weigh the
// points by it. The blobs are counted too, and a point lost or merged fails.
// A cloud whose points run together in the expected picture (fewer blobs than
// points) has no count to judge, and a centroid of its pixels would weigh each
// place by a renderer's dot size (Stanford's drill, 4,223 points in 2 blobs of
// 5,130 and 3,314 px: 4.17 px off in SceneKit and 5.72 px in Storm while the
// boxes agreed within 1.75 px; on a synthetic cloud drawn twice, in the same
// places, as 4 px squares and as 1 px dots, 6.5 to 10.9 px). So its centroid is
// that of the convex hull of the covered pixels' centres: where the cloud lies,
// not how thick its dots are (the same synthetic pair: 0.08 to 0.18 px, and a
// cloud moved by 2, 4 and 8 px read 2.18, 4.18 and 8.05). The hull sees a
// cloud's outline, not its contents, so a dense cloud also passes denseChecks
// (below). The switch from counting to the outline is a cliff: a single pair of
// points that touch in a sparse cloud makes its blobs fewer than its points.
// The area centroid of the convex hull of `points` (Andrew's monotone chain),
// or null where the hull has no area (fewer than three points, or collinear).
function hullCentroid(points) {
  points.sort((p, q) => p[0] - q[0] || p[1] - q[1]);
  const cross = (o, a, b) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
  const chain = (pts) => {
    const c = [];
    for (const p of pts) {
      while (c.length >= 2 && cross(c[c.length - 2], c[c.length - 1], p) <= 0) c.pop();
      c.push(p);
    }
    return c.slice(0, -1);
  };
  const hull = [...chain(points), ...chain([...points].reverse())];
  let area = 0;
  let cx = 0;
  let cy = 0;
  for (let i = 0; i < hull.length; i++) {
    const [x0, y0] = hull[i];
    const [x1, y1] = hull[(i + 1) % hull.length];
    const c = x0 * y1 - x1 * y0;
    area += c;
    cx += (x0 + x1) * c;
    cy += (y0 + y1) * c;
  }
  return Math.abs(area) < 1e-9 ? null : [cx / (3 * area), cy / (3 * area)];
}
// The checks a dense cloud's picture adds to the box and the hull's centroid,
// which see only its outline (erasing the drill's stem, 8 % of its pixels, or
// an interior block, moves the hull's centroid by 0.00 px). `want` is the
// expected picture (SIZE px), `got` the other's, `w` px wide:
//  - precision: how many of ours' covered pixels lie more than 1 px (in the
//    expected picture's pixels) from any covered pixel of the expected picture:
//    a point where the expected has none. A dot's size cannot cause it, as every
//    renderer's dot lies inside the 4 px square the expected picture draws.
//  - coarse occupancy: the covered DENSE_CELL px cells of each picture, with a
//    tolerance of one cell either way: `missing` of the expected picture's cells
//    have no covered cell within one cell of them here (content lost), `extra`
//    of ours have none in the expected picture (content invented).
const DENSE_CELL = 8;
function denseChecks(want, got, w) {
  const n = SIZE;
  const block = w / n;
  const expected = new Uint8Array(n * n);
  for (let i = 0; i < n * n; i++) expected[i] = want[i * 4 + 3] > 0 ? 1 : 0;
  const near = new Uint8Array(n * n);
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      if (!expected[y * n + x]) continue;
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          if (x + dx >= 0 && y + dy >= 0 && x + dx < n && y + dy < n) near[(y + dy) * n + x + dx] = 1;
        }
      }
    }
  }
  const ours = new Uint8Array(n * n);
  let covered = 0;
  let outside = 0;
  for (let y = 0; y < w; y++) {
    for (let x = 0; x < w; x++) {
      if (got[(y * w + x) * 4 + 3] === 0) continue;
      const i = Math.floor(y / block) * n + Math.floor(x / block);
      covered++;
      ours[i] = 1;
      if (!near[i]) outside++;
    }
  }
  const m = n / DENSE_CELL;
  const cells = (flags) => {
    const c = new Uint8Array(m * m);
    for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) if (flags[y * n + x]) c[Math.floor(y / DENSE_CELL) * m + Math.floor(x / DENSE_CELL)] = 1;
    return c;
  };
  const [ce, co] = [cells(expected), cells(ours)];
  const within = (c, x, y) => {
    for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) if (x + dx >= 0 && y + dy >= 0 && x + dx < m && y + dy < m && c[(y + dy) * m + x + dx]) return true;
    return false;
  };
  let total = 0;
  let missing = 0;
  let extra = 0;
  for (let y = 0; y < m; y++) {
    for (let x = 0; x < m; x++) {
      if (ce[y * m + x]) {
        total++;
        if (!within(co, x, y)) missing++;
      }
      if (co[y * m + x] && !within(ce, x, y)) extra++;
    }
  }
  return { covered, outside, cells: total, missing, extra };
}
function coverageOf(px, w = SIZE, blobs = false, hull = false) {
  const s = SIZE / w;
  const at = (v) => (v + 0.5) * s - 0.5;
  let n = 0;
  let x0 = w;
  let y0 = w;
  let x1 = -1;
  let y1 = -1;
  let sx = 0;
  let sy = 0;
  for (let y = 0; y < w; y++) {
    for (let x = 0; x < w; x++) {
      if (px[(y * w + x) * 4 + 3] === 0) continue;
      n++;
      x0 = Math.min(x0, x);
      y0 = Math.min(y0, y);
      x1 = Math.max(x1, x);
      y1 = Math.max(y1, y);
      sx += at(x);
      sy += at(y);
    }
  }
  if (!n) return { pixels: 0 };
  const out = { pixels: n, box: [x0, y0, x1, y1].map(at), centroid: [sx / n, sy / n] };
  // `hull`: the centroid is that of the convex hull of the covered pixels'
  // centres, which a dot's size moves by half a dot at most, where a centroid of
  // the pixels (or of covered cells) weighs each place by it.
  if (hull) {
    const points = [];
    for (let y = 0; y < w; y++) for (let x = 0; x < w; x++) if (px[(y * w + x) * 4 + 3] !== 0) points.push([at(x), at(y)]);
    // Null where the hull has no area: compareWire then judges neither
    // side's centroid, and says so.
    out.centroid = hullCentroid(points);
  }
  if (!blobs) return out;
  const seen = new Uint8Array(w * w);
  const centres = [];
  for (let start = 0; start < w * w; start++) {
    if (seen[start] || px[start * 4 + 3] === 0) continue;
    const stack = [start];
    seen[start] = 1;
    let bx = 0;
    let by = 0;
    let m = 0;
    while (stack.length) {
      const j = stack.pop();
      const jx = j % w;
      const jy = (j - jx) / w;
      bx += at(jx);
      by += at(jy);
      m++;
      for (let dy = -1; dy <= 1; dy++) {
        for (let dx = -1; dx <= 1; dx++) {
          const nx = jx + dx;
          const ny = jy + dy;
          if (nx < 0 || ny < 0 || nx >= w || ny >= w) continue;
          const k = ny * w + nx;
          if (seen[k] || px[k * 4 + 3] === 0) continue;
          seen[k] = 1;
          stack.push(k);
        }
      }
    }
    centres.push([bx / m, by / m]);
  }
  out.blobs = centres.length;
  out.centroid = [centres.reduce((t, c) => t + c[0], 0) / centres.length, centres.reduce((t, c) => t + c[1], 0) / centres.length];
  return out;
}
window.compareWire = async (job) => {
  const want = expectedCache.get(`${job.expectedKey}|${job.mode}|${job.t}`) ?? expectedCache.get(`${job.expectedKey}|${job.mode}|static`);
  if (!want) return { error: 'no expected picture' };
  const w = job.width ?? SIZE;
  let got;
  try {
    got = await loadPixels(job.url, w, w);
  } catch (e) {
    return { error: `could not load ${job.url}: ${e}` };
  }
  let a = coverageOf(want, SIZE, !!job.blobs);
  const standApart = !!job.blobs && a.blobs === job.blobs;
  const dense = !!job.blobs && !standApart;
  const blobsSeen = a.blobs;
  if (dense) a = coverageOf(want, SIZE, false, true);
  const b = coverageOf(got, w, standApart, dense);
  // How the centroid was measured, for the report.
  for (const c of [a, b]) if (c.pixels) c.method = standApart ? 'blobs' : dense ? 'hull' : 'pixels';
  // A dense cloud's expected picture, for the report: how many blobs it has
  // against how many points the file holds.
  if (dense && a.pixels) Object.assign(a, { blobsSeen, points: job.blobs });
  // The hull needs three covered centres that are not in a line. Where either
  // picture has none, neither side's centroid is judged (the box and the
  // content checks still are), and both say so.
  if (dense && a.pixels && b.pixels && (!a.centroid || !b.centroid)) {
    for (const c of [a, b]) Object.assign(c, { centroid: null, method: 'box', cannotJudge: 'centroid: fewer than three covered pixel centres that are not in a line' });
  }
  // The content checks of a dense cloud (denseChecks), computed on ours.
  if (dense && a.pixels && b.pixels) b.checks = denseChecks(want, got, w);
  // Expected red, drawn green, both yellow, at the expected picture's size (a
  // pixel is drawn where any pixel of its block of the picture is covered).
  const overlay = new Uint8ClampedArray(want.length);
  const k = Math.round(w / SIZE);
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      const i = (y * SIZE + x) * 4;
      let g = false;
      for (let dy = 0; dy < k && !g; dy++) for (let dx = 0; dx < k && !g; dx++) g = got[((y * k + dy) * w + x * k + dx) * 4 + 3] > 0;
      const wd = want[i + 3] > 0;
      overlay.set(wd || g ? [wd ? 255 : 0, g ? 255 : 0, 0, 255] : [40, 40, 40, 255], i);
    }
  }
  // `gotDrawn` and `wantDrawn` as compareImage's, for verify.mjs's retry of an empty frame.
  const out = { want: a, got: b, gotDrawn: b.pixels, wantDrawn: a.pixels, image: toPng(new Uint8ClampedArray(got), w, w), overlay: toPng(overlay) };
  if (!a.pixels && !b.pixels) return { ...out, ratio: 0, empty: true };
  if (!a.pixels || !b.pixels || a.blobs !== b.blobs) return { ...out, ratio: 1 };
  const apart = [...a.box.map((v, k) => Math.abs(v - b.box[k])), ...(a.centroid ? a.centroid.map((v, k) => Math.abs(v - b.centroid[k])) : [])];
  // What a dense cloud's content checks found (the reasons, where they failed).
  const c = b.checks;
  const why = [];
  if (c?.outside) why.push(`${c.outside} of ${c.covered} covered pixels lie more than 1 px from any covered pixel of the expected picture`);
  if (c?.missing || c?.extra) {
    why.push(
      `at ${DENSE_CELL} px cells, one cell of tolerance, ${c.missing} of the ${c.cells} cells the expected picture covers have no covered cell near them here` +
        (c.extra ? `, and ${c.extra} cells covered here have none near them in the expected picture` : ''),
    );
  }
  return { ...out, ratio: why.length ? Math.max(1, Math.max(...apart) / SIZE) : Math.max(...apart) / SIZE, apart: apart.map((v) => Math.round(v * 100) / 100), ...(why.length ? { why: why.join('; ') } : {}) };
};

// The cross-renderer lights judge (CAMLIGHT): SceneKit's picture of a Collada
// file, or usdrecord's of a USD file, with one light alone on white Lambert
// surfaces, against the expected `light:<node>` picture. The renderers' units
// and falloff differ (verify.mjs cites each: SceneKit reads a Collada light's
// colour clamped to 1 and no attenuation; Storm draws a sphere light 4x), so
// this compares where the light falls and what colour it is, not how bright:
//  - footprint: a pixel is lit when a channel reaches LIGHT_LIT (8-bit), at
//    an exposure that puts the light's brightest pixel near LIGHT_TARGET; a
//    pixel lit on one side only differs unless one within a pixel of it is
//    lit on the other (edge rasterisation). The ratio is over the pixels lit
//    on either side. A wrong direction, position or outer cone moves it.
//  - colour: on pixels lit on both sides, unclipped (every channel below 250)
//    and bright enough to carry a hue (the largest at least LIGHT_HUE), each
//    side's colour decoded to linear and divided by its largest channel; a
//    pixel differs when a channel is more than LIGHT_CHROMA apart.
//  - brightness: the median ratio of the two sides' linear largest channels
//    over those pixels, with each side's exposure: recorded, not judged.
const LIGHT_LIT = 3;
const LIGHT_HUE = 128;
const LIGHT_CHROMA = 0.06;
window.compareLightPicture = async (job) => {
  const want = expectedCache.get(`${job.expectedKey}|${job.mode}|${job.t}`) ?? expectedCache.get(`${job.expectedKey}|${job.mode}|static`);
  if (!want) return { error: 'no expected picture' };
  let got;
  try {
    got = await loadPixels(job.url);
  } catch (e) {
    return { error: `could not load ${job.url}: ${e}` };
  }
  const lit = (px, i) => px[i + 3] > 0 && Math.max(px[i], px[i + 1], px[i + 2]) >= LIGHT_LIT;
  const litNear = (px, x, y) => {
    for (let dy = -1; dy <= 1; dy++) {
      for (let dx = -1; dx <= 1; dx++) {
        const xx = x + dx;
        const yy = y + dy;
        if (xx >= 0 && yy >= 0 && xx < SIZE && yy < SIZE && lit(px, (yy * SIZE + xx) * 4)) return true;
      }
    }
    return false;
  };
  const diff = new Uint8ClampedArray(want.length);
  const foot = { union: 0, differ: 0, wantLit: 0, gotLit: 0 };
  const hue = { pixels: 0, differ: 0, maxDelta: 0 };
  const ratios = [];
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      const i = (y * SIZE + x) * 4;
      const a = lit(want, i);
      const b = lit(got, i);
      if (a) foot.wantLit++;
      if (b) foot.gotLit++;
      if (!a && !b) continue;
      foot.union++;
      if (a !== b && !(a ? litNear(got, x, y) : litNear(want, x, y))) {
        foot.differ++;
        diff.set([255, 0, 60, 255], i);
        continue;
      }
      const g = 70 + (want[i] + want[i + 1] + want[i + 2]) / 12;
      diff.set([g, g, g, 255], i);
      if (!a || !b) continue;
      const wa = [want[i], want[i + 1], want[i + 2]];
      const wb = [got[i], got[i + 1], got[i + 2]];
      if (Math.max(...wa, ...wb) >= 250 || Math.max(...wa) < LIGHT_HUE || Math.max(...wb) < LIGHT_HUE) continue;
      const la = wa.map((c) => srgbToLinear(c / 255));
      const lb = wb.map((c) => srgbToLinear(c / 255));
      const ma = Math.max(...la);
      const mb = Math.max(...lb);
      const d = Math.max(...la.map((v, k) => Math.abs(v / ma - lb[k] / mb)));
      hue.pixels++;
      hue.maxDelta = Math.max(hue.maxDelta, d);
      if (d > LIGHT_CHROMA) {
        hue.differ++;
        diff.set([40, 90, 255, 255], i);
      }
      ratios.push(mb / ma);
    }
  }
  ratios.sort((p, q) => p - q);
  const pick = (f) => (ratios.length ? Math.round(ratios[Math.min(ratios.length - 1, Math.floor(f * ratios.length))] * 1000) / 1000 : null);
  return {
    footprint: { ...foot, ratio: foot.union ? foot.differ / foot.union : 0 },
    hue: { ...hue, ratio: hue.pixels ? hue.differ / hue.pixels : 0, maxDelta: Math.round(hue.maxDelta * 1000) / 1000 },
    brightness: { pixels: ratios.length, median: pick(0.5), p10: pick(0.1), p90: pick(0.9) },
    image: toPng(new Uint8ClampedArray(got)),
    diff: foot.differ || hue.differ ? toPng(diff) : null,
  };
};

// Render two files of the same model — a third-party original through its own
// loader, and our reading of it — each framed on itself, and compare. `tol` is
// the per-channel tolerance (8-bit) of the static modes' comparison, 20 unless
// given; each result carries `maxDelta`, the largest channel difference.
window.renderPair = async (a, b, cacheKey, tol) => {
  let ra, rb;
  // Ours first: when the native loader throws, a second renderer can still
  // be compared with our reading from the camera framed on it.
  try {
    rb = await renderFile({ ...b, autoFrame: true });
  } catch (e) {
    return { error: `glb loader failed: ${e}` };
  }
  if (cacheKey) {
    for (const mode of b.modes) {
      if (rb.frames[mode]?.static) expectedCache.set(`${cacheKey}|${mode}|static`, rb.frames[mode].static);
    }
    // A reading with points or lines is also kept without them, through the
    // same camera (`<key>#wire`): the picture a renderer that draws triangles
    // alone is compared with (SceneKit, WIRE).
    if (rb.stats.points + rb.stats.lines > 0) {
      const bare = await renderFile({ ...b, blind: 'wire', camera: rb.camera, autoFrame: false, modes: b.modes.filter((m) => !m.startsWith('wire_')) });
      for (const mode of Object.keys(bare.frames)) {
        if (bare.frames[mode]?.static) expectedCache.set(`${cacheKey}#wire|${mode}|static`, bare.frames[mode].static);
      }
    }
  }
  try {
    ra = await renderFile({ ...a, autoFrame: true });
  } catch (e) {
    const ours = {};
    for (const mode of b.modes) if (rb.frames[mode]?.static) ours[mode] = toPng(rb.frames[mode].static);
    return { error: `native ${a.format} loader failed: ${e}`, nativeFailed: true, ourCamera: rb.camera, ourStats: rb.stats, ours };
  }
  const results = {};
  for (const mode of a.modes) {
    const pa = ra.frames[mode]?.static;
    const pb = rb.frames[mode]?.static;
    if (!pa || !pb) continue;
    const cmp = compare(pa, pb, tol ?? 20);
    results[mode] = {
      ratio: cmp.ratio,
      differ: cmp.differ,
      covered: cmp.covered,
      maxDelta: cmp.maxDelta,
      native: toPng(pa),
      ours: toPng(pb),
      diff: cmp.differ ? toPng(cmp.diff) : null,
    };
  }
  // Animated frames too, when the caller asked for them: both files keep the
  // camera framed on their own DEFAULT pose, so a clip read wrongly (a joint
  // turning the wrong way, a key time scaled, a missed pre-rotation) shows as
  // a moved silhouette at the times between keys.
  const animResults = {};
  for (const t of a.times || []) {
    const key = String(t);
    const pa = ra.frames.geom?.[key];
    const pb = rb.frames.geom?.[key];
    if (!pa || !pb) continue;
    const cmp = compare(pa, pb);
    animResults[key] = {
      ratio: cmp.ratio,
      native: toPng(pa),
      ours: toPng(pb),
      diff: cmp.differ ? toPng(cmp.diff) : null,
    };
  }
  return { results, animResults, nativeStats: ra.stats, ourStats: rb.stats, ourCamera: rb.camera };
};

// The native check's lit pass (Ruling A35): a glTF original through its own
// loader (GLTFLoader) and our reading of it (reference.glb), through ONE
// camera (the fixture's), in `lit`, compared at LIT_TOLERANCE; verify.mjs
// judges it at THRESHOLD.lit (0). Each side's layers as loaded are returned
// too. Where the two differ and `restoreRotations` is set, the comparison is
// repeated on our reading with the original's node rotations put back (nodes
// paired by index and name): litegen's glTF reader renormalizes a rotation
// and its GLB writer writes the result in f64, where three.js composes an
// original's f32-rounded, slightly non-unit quaternion as given, so such an
// original comes back turned by about 1e-7 (Khronos ToyCar, task 5b: 1 pixel
// past the tolerance, 0 with its rotations put back). The repeat isolates
// that one cause, and measures it: `largest` is the most any restored
// quaternion differs from ours in one component (q and -q are one rotation).
// verify.mjs excuses a difference only when the repeat closes it and that
// change is a renormalization's size (LIT_ROTATION_BOUND).
window.renderNativeLit = async (job) => {
  const draw = (url, format) => renderFile({ url, format, files: [], camera: job.camera, modes: ['lit'], times: [], level0: job.level0 });
  let ours;
  let nat;
  try {
    ours = await draw(job.ours, 'glb');
  } catch (e) {
    return { error: `glb loader failed: ${e}` };
  }
  try {
    nat = await draw(job.native, job.format);
  } catch (e) {
    return { error: `native ${job.format} loader failed: ${e}` };
  }
  const a = nat.frames.lit.static;
  const b = ours.frames.lit.static;
  const cmp = compare(a, b, LIT_TOLERANCE);
  const out = {
    ratio: cmp.ratio,
    differ: cmp.differ,
    covered: cmp.covered,
    maxDelta: cmp.maxDelta,
    native: toPng(a),
    ours: toPng(b),
    diff: cmp.differ ? toPng(cmp.diff) : null,
    layers: { native: nat.stats.layers, ours: ours.stats.layers },
  };
  if (cmp.differ && job.restoreRotations) out.restored = await withNativeRotations(job, a);
  return out;
};

// A glTF or GLB's JSON (and a GLB's chunks after it).
async function gltfParts(url) {
  const buf = new Uint8Array(await (await fetch(url)).arrayBuffer());
  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  if (buf.length < 20 || view.getUint32(0, true) !== 0x46546c67) return { json: JSON.parse(new TextDecoder().decode(buf)) };
  const jl = view.getUint32(12, true);
  return { json: JSON.parse(new TextDecoder().decode(buf.subarray(20, 20 + jl))), rest: buf.subarray(20 + jl) };
}

// A GLB from a JSON document and the chunks that followed its JSON chunk.
function glbUrl(json, rest) {
  const encoded = new TextEncoder().encode(JSON.stringify(json));
  const text = new Uint8Array(encoded.length + ((4 - (encoded.length % 4)) % 4));
  text.set(encoded);
  text.fill(0x20, encoded.length);
  const head = new Uint8Array(20);
  const hv = new DataView(head.buffer);
  hv.setUint32(0, 0x46546c67, true); // 'glTF'
  hv.setUint32(4, 2, true);
  hv.setUint32(8, 20 + text.length + rest.length, true);
  hv.setUint32(12, text.length, true);
  hv.setUint32(16, 0x4e4f534a, true); // 'JSON'
  return URL.createObjectURL(new Blob([head, text, rest], { type: 'model/gltf-binary' }));
}

// Our reading as a GLB with the original's node rotations put back (nodes
// paired by index and name): { url, rotations, largest } (the caller revokes
// `url`), { rotations: 0 } when none differs, or { error }.
async function oursWithNativeRotations(job) {
  const nat = await gltfParts(job.native);
  const ours = await gltfParts(job.ours);
  const a = nat.json.nodes || [];
  const b = ours.json.nodes || [];
  if (a.length !== b.length || a.some((n, i) => (n.name ?? null) !== (b[i].name ?? null))) return { error: 'the nodes do not pair by index and name' };
  let rotations = 0;
  let largest = 0;
  a.forEach((n, i) => {
    if (n.rotation && JSON.stringify(n.rotation) !== JSON.stringify(b[i].rotation)) {
      const ours = b[i].rotation || [0, 0, 0, 1];
      const apart = (sign) => Math.max(...n.rotation.map((x, k) => Math.abs(x - sign * ours[k])));
      largest = Math.max(largest, Math.min(apart(1), apart(-1)));
      b[i].rotation = n.rotation;
      rotations++;
    }
  });
  if (!rotations) return { rotations: 0 };
  return { url: glbUrl(ours.json, ours.rest), rotations, largest };
}

async function withNativeRotations(job, nativePixels) {
  const o = await oursWithNativeRotations(job);
  if (o.error) return { error: o.error };
  if (!o.rotations) return { rotations: 0 };
  let r;
  try {
    r = await renderFile({ url: o.url, format: 'glb', files: [], camera: job.camera, modes: ['lit'], times: [], level0: job.level0 });
  } finally {
    URL.revokeObjectURL(o.url);
  }
  const cmp = compare(nativePixels, r.frames.lit.static, LIT_TOLERANCE);
  return { rotations: o.rotations, largest: o.largest, ratio: cmp.ratio, differ: cmp.differ, covered: cmp.covered, maxDelta: cmp.maxDelta };
}

// The native check's camera and light passes (item 5's Ruling R24): a glTF
// original and our reading of it (reference.glb), each through GLTFLoader, in
// `camera` (each through its own camera: the original's first, and ours by
// that camera's node name, at the original camera's aspect) and
// `lit_lights` (each lit by its own lights, through the fixture's camera), at
// rest and at the sample times; verify.mjs judges both at 0.00 %. Each side's
// cameras and lights are returned too. Where the two differ and
// `restoreRotations` is set, the comparison is repeated on our reading with
// the original's rotations put back, as renderNativeLit does: `differ` there
// counts the pixels still apart, `largest` the most a rotation moved.
window.renderNativeCamlight = async (job) => {
  const draw = (url, format, extra = {}) => renderFile({ url, format, files: [], camera: job.camera, modes: job.modes, times: job.times || [], clipName: job.clipName, ...extra });
  let nat;
  try {
    nat = await draw(job.native, job.format);
  } catch (e) {
    return { error: `native ${job.format} loader failed: ${e}` };
  }
  const name = nat.stats.cameras[0]?.name;
  const drawOurs = (url) => draw(url, 'glb', { cameraName: name, cameraView: nat.stats.cameraView ?? undefined });
  let ours;
  try {
    ours = await drawOurs(job.ours);
  } catch (e) {
    return { error: `glb loader failed: ${e}` };
  }
  const compareAll = (r) => {
    const results = {};
    let differ = 0;
    for (const mode of job.modes) {
      results[mode] = {};
      const [w, h] = sizeOf(mode, nat.stats);
      for (const [t, a] of Object.entries(nat.frames[mode] || {})) {
        const b = r.frames[mode]?.[t] ?? null;
        if (!a || !b) {
          results[mode][t] = { error: !a ? 'the original has no camera to draw through' : `our reading has no camera on a node named ${name}` };
          differ++;
          continue;
        }
        const cmp = compare(a, b, mode === 'lit_lights' ? LIT_TOLERANCE : 20);
        differ += cmp.differ;
        results[mode][t] = { ratio: cmp.ratio, differ: cmp.differ, covered: cmp.covered, maxDelta: cmp.maxDelta, native: toPng(a, w, h), ours: toPng(b, w, h), diff: cmp.differ ? toPng(cmp.diff, w, h) : null };
      }
    }
    return { results, differ };
  };
  const first = compareAll(ours);
  const out = { results: first.results, cameraName: name, cameras: { native: nat.stats.cameras, ours: ours.stats.cameras }, lights: { native: nat.stats.lights, ours: ours.stats.lights } };
  if (first.differ && job.restoreRotations) {
    const o = await oursWithNativeRotations(job);
    if (o.error || !o.rotations) {
      out.restored = o.error ? { error: o.error } : { rotations: 0 };
    } else {
      try {
        const again = compareAll(await drawOurs(o.url));
        out.restored = { rotations: o.rotations, largest: o.largest, differ: again.differ };
      } finally {
        URL.revokeObjectURL(o.url);
      }
    }
  }
  return out;
};

// Compare a picture rendered elsewhere (SceneKit) with an expected picture.
// silhouette: coverage only — the expected picture's alpha against the
// image's; albedo: colour, as compare() does.
async function loadPixels(url, w = SIZE, h = SIZE) {
  const img = new Image();
  img.src = url;
  await img.decode();
  // A picture of another size is an error, never scaled into this one.
  if (img.naturalWidth !== w || img.naturalHeight !== h) throw new Error(`the picture is ${img.naturalWidth}x${img.naturalHeight}, not ${w}x${h}`);
  const c = document.createElement('canvas');
  c.width = w;
  c.height = h;
  const ctx = c.getContext('2d');
  ctx.drawImage(img, 0, 0, w, h);
  return ctx.getImageData(0, 0, w, h).data;
}

// `opts.wantMode`: the expected picture to compare with, where it is not the
// mode's own (a silhouette through the file's camera is the `camera`
// picture's coverage); `opts.width`, `opts.height`: its size.
window.compareImage = async (url, expectedKey, mode, key, opts = {}) => {
  const wantMode = opts.wantMode ?? (mode === 'silhouette' ? 'albedo' : mode);
  const [w, h] = [opts.width ?? SIZE, opts.height ?? SIZE];
  const want =
    expectedCache.get(`${expectedKey}|${wantMode}|${key}`) ?? expectedCache.get(`${expectedKey}|${wantMode}|static`);
  if (!want) return { error: 'no expected picture' };
  let got;
  try {
    got = await loadPixels(url, w, h);
  } catch (e) {
    return { error: `could not load ${url}: ${e}` };
  }
  // How much each side drew: an empty frame from the other renderer, where
  // a picture is expected, is its failure to draw, not a difference.
  const drawn = (px) => {
    let n = 0;
    for (let i = 3; i < px.length; i += 4) if (px[i] > 0) n++;
    return n;
  };
  const counts = { gotDrawn: drawn(got), wantDrawn: drawn(want) };
  if (mode === 'silhouette') {
    // Coverage only: paint both white where covered.
    const a = new Uint8ClampedArray(want.length);
    const b = new Uint8ClampedArray(got.length);
    for (let i = 0; i < a.length; i += 4) {
      const ca = want[i + 3] > 0 ? 255 : 0;
      const cb = got[i + 3] > 0 ? 255 : 0;
      a[i] = a[i + 1] = a[i + 2] = a[i + 3] = ca;
      b[i] = b[i + 1] = b[i + 2] = b[i + 3] = cb;
    }
    const cmp = compare(a, b);
    return { ratio: cmp.ratio, diff: cmp.differ ? toPng(cmp.diff, w, h) : null, ...counts };
  }
  const cmp = compareNear(want, got, 24, w, h);
  return { ratio: cmp.ratio, diff: cmp.differ ? toPng(cmp.diff, w, h) : null, ...counts };
};

// SceneKit albedo's judge (Ruling A38): the mean colour of each 4x4 block that
// is covered throughout on both sides, compared at the albedo tolerance (24),
// beside compareImage's per-pixel figure, which verify.mjs keeps as
// information. Two renderers minify a texture finer than a pixel
// differently, pixel by pixel: Khronos SheenCloth's fabric, tiled 30 x 30, put
// 6.30 % of SceneKit's pixels past compareNear's tolerance and 0 of its 504
// blocks past this one; DispersionTest's checker 3.67 % and 0 of 943 (task 5b).
// A wrong colour, texture or placement moves whole blocks. `blocks` 0 means
// nothing was judged here (verify.mjs then keeps the per-pixel verdict).
const ALBEDO_BLOCK = 4;
const ALBEDO_BLOCK_TOLERANCE = 24;
window.compareImageBlocks = async (url, expectedKey, key) => {
  const want = expectedCache.get(`${expectedKey}|albedo|${key}`) ?? expectedCache.get(`${expectedKey}|albedo|static`);
  if (!want) return { error: 'no expected picture' };
  let got;
  try {
    got = await loadPixels(url);
  } catch (e) {
    return { error: `could not load ${url}: ${e}` };
  }
  const n = SIZE / ALBEDO_BLOCK;
  const diff = new Uint8ClampedArray(want.length);
  let blocks = 0;
  let differ = 0;
  let maxDelta = 0;
  for (let by = 0; by < n; by++) {
    for (let bx = 0; bx < n; bx++) {
      const sum = [0, 0, 0, 0, 0, 0];
      let k = 0;
      for (let y = 0; y < ALBEDO_BLOCK; y++) {
        for (let x = 0; x < ALBEDO_BLOCK; x++) {
          const i = ((by * ALBEDO_BLOCK + y) * SIZE + bx * ALBEDO_BLOCK + x) * 4;
          if (got[i + 3] === 0 || want[i + 3] === 0) continue;
          k++;
          for (let c = 0; c < 3; c++) {
            sum[c] += got[i + c];
            sum[3 + c] += want[i + c];
          }
        }
      }
      if (k < ALBEDO_BLOCK * ALBEDO_BLOCK) continue;
      blocks++;
      const d = Math.max(...[0, 1, 2].map((c) => Math.abs(sum[c] - sum[3 + c]) / k));
      maxDelta = Math.max(maxDelta, d);
      const bad = d > ALBEDO_BLOCK_TOLERANCE;
      if (bad) differ++;
      for (let y = 0; y < ALBEDO_BLOCK; y++) {
        for (let x = 0; x < ALBEDO_BLOCK; x++) {
          const i = ((by * ALBEDO_BLOCK + y) * SIZE + bx * ALBEDO_BLOCK + x) * 4;
          const g = 70 + (want[i] + want[i + 1] + want[i + 2]) / 12;
          diff.set(bad ? [255, 0, 60, 255] : [g, g, g, 255], i);
        }
      }
    }
  }
  return { ratio: blocks ? differ / blocks : 0, differ, blocks, maxDelta: Math.round(maxDelta * 10) / 10, diff: differ ? toPng(diff) : null };
};

// Cross-renderer colour comparison: a pixel differs only if NO expected pixel
// within one pixel of it matches. Two renderers sample the same texture with
// sub-texel offsets, so every sharp edge in a texture moves by a pixel; this
// absorbs that while a wrong texture, a flipped UV or a wrong colour — which
// differ everywhere, not just along edges — still fail.
function compareNear(want, got, tol, w = SIZE, h = SIZE) {
  let covered = 0;
  let differ = 0;
  const diff = new Uint8ClampedArray(want.length);
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
      if (!ok) {
        differ++;
        diff[i] = 255;
        diff[i + 3] = 255;
      } else {
        const g = 70 + (want[i] + want[i + 1] + want[i + 2]) / 12;
        diff[i] = diff[i + 1] = diff[i + 2] = g;
        diff[i + 3] = 255;
      }
    }
  }
  return { covered, differ, ratio: covered ? differ / covered : 0, diff };
}

// Render a file and return its pictures without comparing them (reports,
// before/after illustrations).
window.renderOnly = async (job) => {
  const r = await renderFile(job);
  const images = {};
  for (const [mode, byT] of Object.entries(r.frames)) {
    images[mode] = {};
    for (const [t, px] of Object.entries(byT)) images[mode][t] = toPng(px);
  }
  return { images, stats: r.stats };
};

// The clips USDLoader (or any loader `load` serves) lists for a file, with their
// durations and track names: what `three_js_lists_every_clip` judges for a
// multi-clip USD file (mesh-gaps item 6, verify.mjs usdClipChecks).
window.listClips = async ({ url, format, files }) => {
  const { clips } = await load(url, format, files || []);
  return clips.map((c) => ({ name: c.name, duration: c.duration, tracks: c.tracks.map((t) => t.name) }));
};

// The two-light picture of a file (mesh-gaps item 6, unlit in USD): the
// loader's own materials under an ambient and a directional light, no
// environment, at each of `levels` (`{ ambient, directional }`, in three.js
// units, where a Lambert surface of albedo a shows a x intensity / pi), with
// only the meshes on a node named one of `only` (or below one) drawn. A
// surface that does not answer to light shows the same pixels at every level.
// Returns a picture per level and what material the loader built for each mesh
// drawn, for the report.
window.renderLightLevels = async ({ url, format, files, camera, only, levels }) => {
  const { root } = await load(url, format, files || []);
  await texturesReady(root);
  const scene = new THREE.Scene();
  scene.add(root);
  const cam = new THREE.PerspectiveCamera(camera.fov, 1, camera.near, camera.far);
  cam.position.fromArray(camera.position);
  cam.lookAt(new THREE.Vector3().fromArray(camera.target));
  cam.updateMatrixWorld(true);
  const shown = [];
  for (const m of drawables(root)) {
    let named = false;
    for (let o = m; o && !named; o = o.parent) named = only.includes(o.name);
    m.visible = named;
    m.frustumCulled = false;
    if (named && m.isMesh) shown.push(m);
  }
  scene.environment = null;
  const ambient = new THREE.AmbientLight(0xffffff, 1);
  const sun = new THREE.DirectionalLight(0xffffff, 1);
  // From the camera's upper left, in front of the subject.
  const column = (i) => new THREE.Vector3().setFromMatrixColumn(cam.matrixWorld, i);
  const target = new THREE.Vector3().fromArray(camera.target);
  const away = cam.position.distanceTo(target);
  sun.position.copy(target).addScaledVector(column(0), -0.5 * away).addScaledVector(column(1), 0.7 * away).addScaledVector(column(2), 0.8 * away);
  sun.target.position.copy(target);
  scene.add(ambient, sun, sun.target);
  sun.target.updateMatrixWorld(true);
  root.updateMatrixWorld(true);
  const frames = [];
  for (const level of levels) {
    ambient.intensity = level.ambient;
    sun.intensity = level.directional;
    renderer.render(scene, cam);
    frames.push({ level, image: toPng(capture()) });
  }
  const color = (c) => (c ? [c.r, c.g, c.b].map((v) => Math.round(v * 1000) / 1000) : null);
  const materials = shown.map((m) => {
    const x = Array.isArray(m.material) ? m.material[0] : m.material;
    return {
      mesh: m.name,
      type: x.type,
      color: color(x.color),
      emissive: color(x.emissive),
      emissiveMap: !!x.emissiveMap,
      map: !!x.map,
      metalness: x.metalness ?? null,
      roughness: x.roughness ?? null,
    };
  });
  scene.remove(root, ambient, sun, sun.target);
  return { frames, materials };
};

window.ready = true;
