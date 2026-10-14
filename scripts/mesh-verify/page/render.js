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

import * as THREE from 'three';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';
import { OBJLoader } from 'three/addons/loaders/OBJLoader.js';
import { MTLLoader } from 'three/addons/loaders/MTLLoader.js';
import { STLLoader } from 'three/addons/loaders/STLLoader.js';
import { PLYLoader } from 'three/addons/loaders/PLYLoader.js';
import { FBXLoader } from 'three/addons/loaders/FBXLoader.js';
import { ColladaLoader } from 'three/addons/loaders/ColladaLoader.js';
import { ThreeMFLoader } from 'three/addons/loaders/3MFLoader.js';
import { USDLoader } from 'three/addons/loaders/USDLoader.js';

const SIZE = 256;

const renderer = new THREE.WebGLRenderer({ antialias: false, preserveDrawingBuffer: true, alpha: true });
renderer.setPixelRatio(1);
renderer.setSize(SIZE, SIZE);
renderer.setClearColor(0x000000, 0);
renderer.outputColorSpace = THREE.SRGBColorSpace;
renderer.toneMapping = THREE.NoToneMapping;
document.body.appendChild(renderer.domElement);

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
function usdLimitation(format, mode, t, info) {
  if (!USD_TEXT_ROOT[format] && format !== 'usdc' && format !== 'usd') return null;
  if (mode === 'albedo') {
    return 'three.js USDLoader decodes linear UsdPreviewSurface colours as sRGB and keeps only displayColor[0]; albedo verified by usdrecord';
  }
  if (info.morphed) return 'three.js USDLoader has no BlendShape support; verified by usdrecord silhouettes';
  if (info.singularBone && t === 'static') {
    return 'three.js Matrix4.decompose turns a zero-scale USD rest joint into unit scale; verified by usdrecord';
  }
  if (info.skinned && t !== 'static' && USD_TEXT_ROOT[format]) {
    return 'three.js USDAParser does not reorder quatf[] (w,x,y,z); skeletal animation verified by usdrecord and by a usdcat crate copy';
  }
  return null;
}

function loaderFor(format) {
  switch (format) {
    case 'glb':
    case 'gltf':
      return new GLTFLoader();
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
    return { root: result.scene, clips: result.animations || [] };
  }
  if (format === 'dae') {
    return { root: result.scene, clips: result.scene.animations || result.animations || [] };
  }
  if (result.isBufferGeometry) {
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
      if (!o.isMesh) return;
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
const flippedUv = new WeakSet();
function setUvFlip(mesh, flipped) {
  const uv = mesh.geometry.attributes.uv;
  if (!uv || flippedUv.has(uv) === flipped) return;
  const a = uv.array;
  for (let i = 1; i < a.length; i += 2) a[i] = 1 - a[i];
  uv.needsUpdate = true;
  if (flipped) flippedUv.add(uv);
  else flippedUv.delete(uv);
}

function applyMode(root, mode, format) {
  for (const mesh of meshes(root)) {
    if (V_UP[format]) setUvFlip(mesh, mode === 'uv');
    if (!mesh.userData.original) mesh.userData.original = mesh.material;
    const orig = mesh.userData.original;
    const pick = (fn) => (Array.isArray(orig) ? orig.map((m) => fn(m)) : fn(orig));
    mesh.userData.modeMaterials = mesh.userData.modeMaterials || {};
    if (mesh.userData.modeMaterials[mode]) {
      mesh.material = mesh.userData.modeMaterials[mode];
      continue;
    }
    switch (mode) {
      case 'geom':
        mesh.material = pick(() => new THREE.MeshNormalMaterial({ flatShading: true, side: THREE.FrontSide }));
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
        // Coverage only, both sides: what usdrecord's alpha shows (Storm
        // draws back faces; the other modes cull them).
        mesh.material = pick(() => new THREE.MeshBasicMaterial({ color: 0xffffff, side: THREE.DoubleSide }));
        break;
      case 'uv': {
        const uv = mesh.geometry.attributes.uv;
        mesh.material = pick(() => new THREE.MeshBasicMaterial({ map: uv ? UV_TEXTURE : null, color: uv ? 0xffffff : 0xff00ff, side: THREE.FrontSide }));
        break;
      }
    }
    mesh.userData.modeMaterials[mode] = mesh.material;
  }
}

function capture() {
  const gl = renderer.getContext();
  const px = new Uint8Array(SIZE * SIZE * 4);
  gl.readPixels(0, 0, SIZE, SIZE, gl.RGBA, gl.UNSIGNED_BYTE, px);
  // readPixels is bottom-up; flip to top-down for images and diffs.
  const out = new Uint8ClampedArray(SIZE * SIZE * 4);
  for (let y = 0; y < SIZE; y++) {
    out.set(px.subarray((SIZE - 1 - y) * SIZE * 4, (SIZE - y) * SIZE * 4), y * SIZE * 4);
  }
  return out;
}

function toPng(pixels) {
  const c = document.createElement('canvas');
  c.width = SIZE;
  c.height = SIZE;
  const ctx = c.getContext('2d');
  ctx.putImageData(new ImageData(pixels, SIZE, SIZE), 0, 0);
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
function frameCamera(root) {
  root.updateMatrixWorld(true);
  const box = new THREE.Box3().setFromObject(root, true);
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

async function renderFile({ url, format, files, camera, modes, times, clipName, autoFrame }) {
  const { root, clips } = await load(url, format, files);
  const texturesLoaded = await texturesReady(root);
  // Taken at rest, before any clip moves a bone.
  const singularBone = meshes(root).some(
    (m) => m.isSkinnedMesh && m.skeleton.bones.some((b) => b.scale.x === 0 || b.scale.y === 0 || b.scale.z === 0),
  );
  const scene = new THREE.Scene();
  scene.add(root);
  if (autoFrame) camera = frameCamera(root);
  const cam = new THREE.PerspectiveCamera(camera.fov, 1, camera.near, camera.far);
  cam.position.fromArray(camera.position);
  cam.lookAt(new THREE.Vector3().fromArray(camera.target));
  cam.updateMatrixWorld(true);

  // Default morph weights, read before any clip moves them (GLTFLoader
  // starts each mesh at its `weights`): see FBX_STATIC_MORPH below.
  const morphDefaults = meshes(root).some((m) => (m.morphTargetInfluences || []).some((w) => w !== 0));
  let mixer = null;
  let clip = null;
  if (times && times.length && clips.length) {
    clip = clips.find((c) => c.name === clipName) || clips[0];
    mixer = new THREE.AnimationMixer(root);
    // USD has one timeline, which USDLoader splits into a clip per kind of
    // data: a SkelAnimation's ("Anim") and the xformOps'
    // ("TransformAnimation"). Played alone, the first leaves every animated
    // xform at rest. So for USD, all of them play.
    const playing = USD_ANY[format] ? clips : [clip];
    for (const c of playing) {
      const action = mixer.clipAction(c);
      // LoopOnce + clamp, or the final sample wraps to frame 0.
      action.setLoop(THREE.LoopOnce, 1);
      action.clampWhenFinished = true;
      action.play();
    }
  }
  for (const m of meshes(root)) m.frustumCulled = false;
  // The default pose of every mode first: once the mixer has moved the
  // skeleton there is no cheap way back to it.
  const out = {};
  for (const mode of modes) {
    applyMode(root, mode, format);
    root.updateMatrixWorld(true);
    renderer.render(scene, cam);
    out[mode] = { static: capture() };
  }
  if (mixer) {
    for (const t of times) {
      mixer.setTime(t);
      for (const mode of modes) {
        applyMode(root, mode, format);
        root.updateMatrixWorld(true);
        renderer.render(scene, cam);
        out[mode][String(t)] = capture();
      }
    }
  }
  const stats = { meshes: meshes(root).length, clips: clips.map((c) => `${c.name}:${c.duration.toFixed(3)}s`), animated: !!mixer, texturesLoaded, morphDefaults };
  let triangles = 0;
  for (const m of meshes(root)) {
    const g = m.geometry;
    triangles += g.index ? g.index.count / 3 : g.attributes.position.count / 3;
  }
  stats.triangles = triangles;
  // Whether the file itself carries normals / UVs on every mesh. Loaders
  // disagree about inventing them (OBJLoader computes normals when a file has
  // none; GLTFLoader does not), so a mode is only compared when the EXPECTED
  // file really has the data.
  stats.hasNormals = meshes(root).every((m) => !!m.geometry.attributes.normal);
  stats.hasUv = meshes(root).every((m) => !!m.geometry.attributes.uv);
  stats.skinned = meshes(root).some((m) => m.isSkinnedMesh);
  stats.singularBone = singularBone;
  stats.morphed = meshes(root).some((m) => (m.geometry.morphAttributes.position || []).length > 0);
  // Morph targets that move normals as well as positions (see
  // FBX_MORPH_NORMALS).
  stats.morphNormals = meshes(root).some((m) => (m.geometry.morphAttributes.normal || []).length > 0);
  scene.remove(root);
  return { frames: out, stats, camera };
}

window.renderExpected = async (job) => {
  const r = await renderFile(job);
  expectedInfo.set(job.key, r.stats);
  const images = {};
  for (const [mode, byT] of Object.entries(r.frames)) {
    images[mode] = {};
    for (const [t, px] of Object.entries(byT)) {
      expectedCache.set(`${job.key}|${mode}|${t}`, px);
      images[mode][t] = toPng(px);
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

window.renderCase = async (job) => {
  let r;
  try {
    r = await renderFile(job);
  } catch (e) {
    return { error: String(e && e.stack ? e.stack.split('\n').slice(0, 3).join(' | ') : e) };
  }
  const results = {};
  const keys = ['static', ...(job.times || []).map(String)];
  const info = expectedInfo.get(job.expectedKey) || {};
  for (const [mode, byT] of Object.entries(r.frames)) {
    results[mode] = {};
    // A limitation that holds at every time skips the mode outright.
    const always = usdLimitation(job.format, mode, 'static', info);
    if (always && usdLimitation(job.format, mode, 'animated', info)) {
      results[mode].static = { skipped: always };
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
      const limit = usdLimitation(job.format, mode, t, info);
      if (limit) {
        results[mode][t] = { skipped: limit };
        continue;
      }
      // A file that lost its animation shows its default pose at every time,
      // and is compared as such: the frames that should have moved fail.
      const px = byT[t] ?? byT.static;
      // An unanimated expected picture holds its default pose at every time.
      const want =
        expectedCache.get(`${job.expectedKey}|${mode}|${t}`) ?? expectedCache.get(`${job.expectedKey}|${mode}|static`);
      if (!want) {
        results[mode][t] = { error: 'no expected picture' };
        continue;
      }
      const cmp = compare(want, px);
      results[mode][t] = {
        ratio: cmp.ratio,
        differ: cmp.differ,
        covered: cmp.covered,
        maxDelta: cmp.maxDelta,
        image: toPng(px),
        diff: cmp.differ ? toPng(cmp.diff) : null,
      };
    }
  }
  return { results, stats: r.stats };
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
  const c = document.createElement('canvas');
  c.width = SIZE;
  c.height = SIZE;
  const ctx = c.getContext('2d');
  ctx.drawImage(img, 0, 0, SIZE, SIZE);
  const px = ctx.getImageData(0, 0, SIZE, SIZE).data;
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
          if (xx < 0 || yy < 0 || xx >= SIZE || yy >= SIZE) continue;
          if (covered(img, (yy * SIZE + xx) * 4) === want1) return true;
        }
      }
      return false;
    };
    for (let y = 0; y < SIZE; y++) {
      for (let x = 0; x < SIZE; x++) {
        const i = (y * SIZE + x) * 4;
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
        return xx >= 0 && yy >= 0 && xx < SIZE && yy < SIZE && covered(a, (yy * SIZE + xx) * 4);
      });
    for (let y = 0; y < SIZE; y++) {
      for (let x = 0; x < SIZE; x++) {
        const i = (y * SIZE + x) * 4;
        if (!covered(want, i) || !covered(px, i) || !interior(want, x, y) || !interior(px, x, y)) continue;
        total++;
        let ok = false;
        for (let dy = -1; dy <= 1 && !ok; dy++) {
          for (let dx = -1; dx <= 1 && !ok; dx++) {
            const xx = x + dx;
            const yy = y + dy;
            if (xx < 0 || yy < 0 || xx >= SIZE || yy >= SIZE) continue;
            const j = (yy * SIZE + xx) * 4;
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
  return { ratio: total ? differ / total : 0, differ, covered: total, image: toPng(new Uint8ClampedArray(px)), diff: differ ? toPng(diff) : null };
};

// Render two files of the same model — a third-party original through its own
// loader, and our reading of it — each framed on itself, and compare.
window.renderPair = async (a, b, cacheKey) => {
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
    const cmp = compare(pa, pb);
    results[mode] = {
      ratio: cmp.ratio,
      differ: cmp.differ,
      covered: cmp.covered,
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

// Compare a picture rendered elsewhere (SceneKit) with an expected picture.
// silhouette: coverage only — the expected picture's alpha against the
// image's; albedo: colour, as compare() does.
async function loadPixels(url) {
  const img = new Image();
  img.src = url;
  await img.decode();
  const c = document.createElement('canvas');
  c.width = SIZE;
  c.height = SIZE;
  const ctx = c.getContext('2d');
  ctx.drawImage(img, 0, 0, SIZE, SIZE);
  return ctx.getImageData(0, 0, SIZE, SIZE).data;
}

window.compareImage = async (url, expectedKey, mode, key) => {
  const wantMode = mode === 'silhouette' ? 'albedo' : mode;
  const want =
    expectedCache.get(`${expectedKey}|${wantMode}|${key}`) ?? expectedCache.get(`${expectedKey}|${wantMode}|static`);
  if (!want) return { error: 'no expected picture' };
  let got;
  try {
    got = await loadPixels(url);
  } catch (e) {
    return { error: `could not load ${url}: ${e}` };
  }
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
    return { ratio: cmp.ratio, diff: cmp.differ ? toPng(cmp.diff) : null };
  }
  const cmp = compareNear(want, got, 24);
  return { ratio: cmp.ratio, diff: cmp.differ ? toPng(cmp.diff) : null };
};

// Cross-renderer colour comparison: a pixel differs only if NO expected pixel
// within one pixel of it matches. Two renderers sample the same texture with
// sub-texel offsets, so every sharp edge in a texture moves by a pixel; this
// absorbs that while a wrong texture, a flipped UV or a wrong colour — which
// differ everywhere, not just along edges — still fail.
function compareNear(want, got, tol) {
  let covered = 0;
  let differ = 0;
  const diff = new Uint8ClampedArray(want.length);
  const at = (x, y) => (y * SIZE + x) * 4;
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
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
          if (xx < 0 || yy < 0 || xx >= SIZE || yy >= SIZE) continue;
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

window.ready = true;
