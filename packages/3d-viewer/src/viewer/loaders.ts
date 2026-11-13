// Loads a model with three.js's own loader for its format, so what the
// viewer shows is what an independent reader makes of the file, not litegen's
// own reading of it.
//
// Ported from scripts/mesh-verify/page/render.js (`loaderFor`, `load`,
// `texturesReady`, lines 259-372), which the render harness has proven
// against every format, with these changes:
//   - files come from memory: every reference a loader makes is resolved
//     against the model's own path, then by a unique case-insensitive file
//     name (a flat browser drop);
//   - the OBJ's material library is the file whose role is
//     `material_library`, as the caller reports it, not one found by name;
//   - the 3MF turn is the general rule for a file stored +Z up;
//   - a text USD layer's rotation keys are reordered (fixTextUsdRotations).
import * as THREE from 'three';
import type { StoredFrame, UpAxis, ViewerFile, ViewerFormat } from '../types';

export interface Loaded {
  root: THREE.Object3D;
  clips: THREE.AnimationClip[];
}

/** A host that is never contacted: `.invalid` is reserved (RFC 2606). Every
 *  URL under it is mapped back to an in-memory file before any fetch. */
const BASE = 'https://files.invalid/';

const normalize = (name: string) => name.replace(/\\/g, '/').replace(/^(\.\/|\/)+/, '');
const basename = (p: string) => p.slice(p.lastIndexOf('/') + 1);

/** Serves `files` to three.js's loaders from blob URLs, and counts the
 *  loads they have in flight. */
export class FileServer {
  readonly manager = new THREE.LoadingManager();
  private readonly urls = new Map<string, string>();
  private pending = 0;

  constructor(files: ViewerFile[]) {
    for (const f of files) {
      this.urls.set(normalize(f.name), URL.createObjectURL(new Blob([f.data as Uint8Array<ArrayBuffer>])));
    }
    this.manager.setURLModifier((url) => this.resolve(url));
    // Every loader reports each file it fetches to its manager: started,
    // then ended, whether it loaded or failed.
    const { itemStart, itemEnd } = this.manager;
    this.manager.itemStart = (url: string) => {
      this.pending += 1;
      itemStart(url);
    };
    this.manager.itemEnd = (url: string) => {
      this.pending = Math.max(0, this.pending - 1);
      itemEnd(url);
    };
  }

  /** Resolves once every load the loaders started has ended, loaded or
   *  failed: MTLLoader, FBXLoader, ColladaLoader and the 3MF loader hand the
   *  model back first and fetch its textures after. A texture that is not
   *  among the files fails at once, so the model draws untextured instead of
   *  waiting for an image that will never come. False when `timeoutMs`
   *  passes first, or when `cancelled` says the load is no longer wanted. */
  async settled(timeoutMs = 15_000, cancelled: () => boolean = () => false): Promise<boolean> {
    const start = performance.now();
    for (;;) {
      if (cancelled()) return false;
      if (this.pending === 0) return true;
      if (performance.now() - start > timeoutMs) return false;
      await new Promise((r) => setTimeout(r, 25));
    }
  }

  /** The URL a loader is given for `name`: under BASE, so relative
   *  references resolve against the file's own folder. */
  urlOf(name: string): string {
    return BASE + normalize(name).split('/').map(encodeURIComponent).join('/');
  }

  private resolve(url: string): string {
    if (url.startsWith('data:') || url.startsWith('blob:')) return url;
    const path = url.startsWith(BASE) ? decodeURIComponent(url.slice(BASE.length)) : decodeURIComponent(url);
    const exact = this.urls.get(normalize(path));
    if (exact) return exact;
    const wanted = basename(normalize(path)).toLowerCase();
    const named = [...this.urls].filter(([name]) => basename(name).toLowerCase() === wanted);
    // A reference to a file that was not given resolves to nothing, so the
    // loader reports it (or draws the surface untextured) at once.
    return named.length === 1 ? named[0]![1] : 'data:,';
  }

  dispose(): void {
    for (const url of this.urls.values()) URL.revokeObjectURL(url);
  }
}

/** The quarter turn about X that brings a +Z-up file into three.js's +Y up:
 *  a format stored +Z up (3MF Core §3.1), or a unitless one the caller says
 *  was written +Z up. Loaders that read a declared axis (Collada) apply it
 *  themselves. */
export function needsZUpTurn(storedFrame: StoredFrame | undefined, up: UpAxis | undefined): boolean {
  return storedFrame === 'z_up' || (storedFrame === 'caller' && up === 'z');
}

const SURFACE = new THREE.MeshStandardMaterial({ color: 0xb8bcc4, metalness: 0, roughness: 0.65 });

/** STL and PLY give bare geometry: a neutral surface, with the vertex
 *  colours a PLY carries. */
function meshOf(geometry: THREE.BufferGeometry): THREE.Mesh {
  if (!geometry.getAttribute('normal')) geometry.computeVertexNormals();
  const material = geometry.getAttribute('color') ? new THREE.MeshStandardMaterial({ vertexColors: true, metalness: 0, roughness: 0.65 }) : SURFACE;
  return new THREE.Mesh(geometry, material);
}

/** Whether a USD file's root layer is text (`#usda`) rather than a binary
 *  crate: a `.usda` or `.usdc` file by its first bytes (either name reads
 *  either encoding), a USDZ by its first entry, which the USDZ specification
 *  makes the root layer. */
export function usdRootIsText(format: ViewerFormat, data: Uint8Array): boolean {
  if (format === 'usda' || format === 'usdc') return new TextDecoder().decode(data.subarray(0, 5)) === '#usda';
  if (format !== 'usdz' || data.length < 30) return false;
  const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
  if (view.getUint32(0, true) !== 0x04034b50) return false;
  const nameLength = view.getUint16(26, true);
  return new TextDecoder().decode(data.subarray(30, 30 + nameLength)).toLowerCase().endsWith('.usda');
}

/** three.js r183's USD loader takes a SkelAnimation's `quatf` keys as
 *  (x, y, z, w) in every layer. That is a binary crate's memory order, but USD
 *  text writes a quaternion real first, (w, x, y, z): Pixar's usdcat reads
 *  `(1, 0, 0, 0)` as the identity (litegen's mesh test data, usd/stress.usda).
 *  From a text layer, every joint would turn 180 degrees about x. Rotation
 *  keys read from text are put back in three.js's order, in place. */
export function fixTextUsdRotations(clips: THREE.AnimationClip[]): void {
  for (const clip of clips) {
    for (const track of clip.tracks) {
      if (!(track instanceof THREE.QuaternionKeyframeTrack)) continue;
      const v = track.values;
      for (let i = 0; i + 3 < v.length; i += 4) {
        const real = v[i]!;
        v[i] = v[i + 1]!;
        v[i + 1] = v[i + 2]!;
        v[i + 2] = v[i + 3]!;
        v[i + 3] = real;
      }
    }
  }
}

/** Turns on vertex colours for every mesh whose geometry carries them.
 *  three.js r183's ColladaLoader reads a `COLOR` input into the geometry
 *  but leaves its materials' `vertexColors` off, so a coloured Collada file
 *  would draw in its material's colour alone. */
export function showVertexColors(root: THREE.Object3D): void {
  root.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (!mesh.isMesh || !mesh.geometry.getAttribute('color')) return;
    for (const m of Array.isArray(mesh.material) ? mesh.material : [mesh.material]) {
      if (!m.vertexColors) {
        m.vertexColors = true;
        m.needsUpdate = true;
      }
    }
  });
}

/** A loader's own code could not be downloaded: most often a page open
 *  across a deploy, whose old chunks are gone. Not the file's fault, and
 *  retrying in the same page cannot help (browsers keep a failed module
 *  import), so the viewer says to reload. */
export class LoaderUnavailableError extends Error {
  constructor(format: ViewerFormat, cause: unknown) {
    super(`The viewer's ${format.toUpperCase()} loader could not be downloaded (${cause instanceof Error ? cause.message : String(cause)}).`);
    this.name = 'LoaderUnavailableError';
  }
}

async function loaderCode<T>(format: ViewerFormat, load: () => Promise<T>): Promise<T> {
  try {
    return await load();
  } catch (e) {
    throw new LoaderUnavailableError(format, e);
  }
}

/** Each loader is its own chunk, fetched the first time its format is shown. */
export async function loadModel(files: ViewerFile[], format: ViewerFormat, server: FileServer): Promise<Loaded> {
  const main = files.find((f) => f.role === 'model') ?? files[0];
  if (!main) throw new Error('no file to show');
  const url = server.urlOf(main.name);
  const manager = server.manager;
  switch (format) {
    case 'glb':
    case 'gltf': {
      const { GLTFLoader } = await loaderCode(format, () => import('three/addons/loaders/GLTFLoader.js'));
      const gltf = await new GLTFLoader(manager).loadAsync(url);
      return { root: gltf.scene, clips: gltf.animations };
    }
    case 'obj': {
      const [{ OBJLoader }, { MTLLoader }] = await loaderCode(format, () => Promise.all([import('three/addons/loaders/OBJLoader.js'), import('three/addons/loaders/MTLLoader.js')]));
      const loader = new OBJLoader(manager);
      const mtl = files.find((f) => f.role === 'material_library');
      if (mtl) {
        const materials = await new MTLLoader(manager).loadAsync(server.urlOf(mtl.name));
        materials.preload();
        loader.setMaterials(materials);
      }
      return { root: await loader.loadAsync(url), clips: [] };
    }
    case 'stl': {
      const { STLLoader } = await loaderCode(format, () => import('three/addons/loaders/STLLoader.js'));
      return { root: meshOf(await new STLLoader(manager).loadAsync(url)), clips: [] };
    }
    case 'ply': {
      const { PLYLoader } = await loaderCode(format, () => import('three/addons/loaders/PLYLoader.js'));
      return { root: meshOf(await new PLYLoader(manager).loadAsync(url)), clips: [] };
    }
    case 'fbx': {
      const { FBXLoader } = await loaderCode(format, () => import('three/addons/loaders/FBXLoader.js'));
      const root = await new FBXLoader(manager).loadAsync(url);
      return { root, clips: root.animations };
    }
    case 'dae': {
      const { ColladaLoader } = await loaderCode(format, () => import('three/addons/loaders/ColladaLoader.js'));
      const collada = await new ColladaLoader(manager).loadAsync(url);
      if (!collada) throw new Error('ColladaLoader returned nothing');
      showVertexColors(collada.scene);
      return { root: collada.scene, clips: collada.scene.animations };
    }
    case '3mf': {
      const { ThreeMFLoader } = await loaderCode(format, () => import('three/addons/loaders/3MFLoader.js'));
      return { root: await new ThreeMFLoader(manager).loadAsync(url), clips: [] };
    }
    case 'usdz':
    case 'usda':
    case 'usdc': {
      const { USDLoader } = await loaderCode(format, () => import('three/addons/loaders/USDLoader.js'));
      const root = await new USDLoader(manager).loadAsync(url);
      const clips = root.animations ?? [];
      if (usdRootIsText(format, main.data)) fixTextUsdRotations(clips);
      return { root, clips };
    }
    default: {
      const unreachable: never = format;
      throw new Error(`no loader for ${String(unreachable)}`);
    }
  }
}
