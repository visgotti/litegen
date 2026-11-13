/**
 * Pure helpers for 3D generation results. Kept out of the component files so
 * they are unit-testable without a DOM and so the component modules export
 * only components (react-refresh requires that for hot reload). They map the
 * API's assets and scene manifest onto the shared viewer package's shapes
 * (@litegen/3d-viewer); sizes and dimensions are formatted there.
 */
import type { Generation, Model3dAsset } from '@litegen/sdk';
// Type-only from the package's main entry (erased), so three.js stays out of
// the dashboard's first chunk: the components load on first use.
import type { ClipRow } from '@litegen/3d-viewer';
import { isViewerFormat, type Dimensions } from '@litegen/3d-viewer/helpers';

const KINDS = new Set<Model3dAsset['kind']>(['mesh', 'texture', 'preview', 'original', 'package']);

/** `metadata` is arbitrary JSON on the wire, so every entry is checked before
 *  the renderer dereferences it — one malformed row must not blank the page
 *  (the dashboard has no error boundary, so a throw in render unmounts it).
 *  Every field the renderer calls a method on is checked here. */
function isAsset(v: unknown): v is Model3dAsset {
  if (typeof v !== 'object' || v === null) return false;
  const a = v as Record<string, unknown>;
  return typeof a.url === 'string' && a.url !== ''
    && typeof a.format === 'string'
    && KINDS.has(a.kind as Model3dAsset['kind']);
}

/** The well-formed entries of an untrusted asset list (`[]` for non-arrays).
 *  Applied at the inspector's boundary too, because Playground history is
 *  replayed from localStorage and never passes through `assetsOf`. */
export function validAssets(raw: unknown): Model3dAsset[] {
  return Array.isArray(raw) ? raw.filter(isAsset) : [];
}

/** Lowercased file extension of a URL's path ('' when it has none). */
export function extensionOf(url: string): string {
  const path = url.split(/[?#]/, 1)[0];
  const name = path.slice(path.lastIndexOf('/') + 1);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/**
 * The primary mesh. `fallbackUrl` is a generation's `result_url`, which the
 * API also sets to the mesh, so a row written before `assets[]` existed still
 * renders. A listed mesh always wins over the fallback.
 */
export function meshOf(assets: Model3dAsset[], fallbackUrl?: string | null): Model3dAsset | null {
  const meshes = assets.filter(a => a.kind === 'mesh');
  // A generation can carry one mesh per requested container. glb first,
  // exactly like `Model3dGenerationResponse::mesh()` on the backend (which
  // picks the same asset for `result_url`); then a gltf, which can arrive
  // beside it now that litegen converts to it; then the first the shared
  // viewer has a loader for; then whatever is first, so the asset table and
  // the download links still work.
  const format = (a: Model3dAsset) => a.format.toLowerCase();
  const listed = meshes.find(a => format(a) === 'glb')
    ?? meshes.find(a => format(a) === 'gltf')
    ?? meshes.find(a => isViewerFormat(format(a)))
    ?? meshes[0];
  if (listed) return listed;
  if (!fallbackUrl) return null;
  return { kind: 'mesh', url: fallbackUrl, format: extensionOf(fallbackUrl) };
}

/** A generation row's assets (`metadata.assets`), with the mesh backfilled
 *  from `result_url` for older 3D rows. Never throws on malformed metadata. */
export function assetsOf(g: Generation): Model3dAsset[] {
  const assets = validAssets((g.metadata as { assets?: unknown } | null | undefined)?.assets);
  // Only a 3D row's result_url is a mesh; for an image row it is the image.
  if (g.media_type !== 'model3d' || assets.some(a => a.kind === 'mesh')) return assets;
  const mesh = meshOf(assets, g.result_url);
  return mesh ? [mesh, ...assets] : assets;
}

/** The 2D preview render. Null when the provider returned none. */
export function previewOf(assets: Model3dAsset[]): Model3dAsset | null {
  return assets.find(a => a.kind === 'preview') ?? null;
}

/** Texture maps only — the mesh and preview have their own places in the UI. */
export function texturesOf(assets: Model3dAsset[]): Model3dAsset[] {
  return assets.filter(a => a.kind === 'texture');
}

const IMAGE_FORMATS = new Set(['png', 'jpg', 'jpeg', 'webp', 'gif', 'avif']);

/** Whether an asset of this format can be shown in a plain `<img>` (a KTX2
 *  or EXR texture is downloadable but not previewable). */
export function isBrowserImage(format: string): boolean {
  return IMAGE_FORMATS.has(format.toLowerCase());
}

// ─── The scene manifest (3D doc §3D): what litegen measured on delivery ─────

type Loose = Record<string, unknown>;
const isObj = (v: unknown): v is Loose => v != null && typeof v === 'object' && !Array.isArray(v);
const finite = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const nums = (v: unknown): number[] => (Array.isArray(v) ? v.filter((x): x is number => typeof x === 'number' && Number.isFinite(x)) : []);

/**
 * A delivered mesh's manifest as label/value rows — its frame, size,
 * materials, bounds, skin, parts and glTF extensions — or none when it
 * carries no readable one (history replays stored bodies of any shape).
 */
export function sceneFacts(scene: unknown): Array<[string, string]> {
  if (!isObj(scene) || typeof scene.units !== 'string') return [];
  const rows: Array<[string, string]> = [];
  const oriented = typeof scene.oriented_by === 'string' && scene.oriented_by ? ` (oriented by ${scene.oriented_by})` : '';
  rows.push(['Units / up / front', `${scene.units} · ${String(scene.up ?? '—')} · ${String(scene.front ?? '—')}${oriented}`]);
  const tris = finite(scene.triangle_count);
  const verts = finite(scene.vertex_count);
  if (tris != null && verts != null) {
    rows.push(['Triangles / vertices', `${tris.toLocaleString('en-US')} · ${verts.toLocaleString('en-US')}`]);
  }
  const materials = Array.isArray(scene.materials) ? scene.materials.length : 0;
  const uv = finite(scene.uv_sets) ?? 0;
  rows.push(['Materials', `${materials} · ${uv} UV set${uv === 1 ? '' : 's'} · ${scene.vertex_colors === true ? 'vertex colours' : 'no vertex colours'}`]);
  if (isObj(scene.bounds)) {
    const min = nums(scene.bounds.min);
    const max = nums(scene.bounds.max);
    if (min.length === 3 && max.length === 3) {
      rows.push(['Bounds', `${max.map((m, i) => (m - min[i]).toFixed(2)).join(' × ')} ${scene.units}`]);
    }
  }
  if (isObj(scene.skin)) {
    const joints = finite(scene.skin.joint_count);
    const parts = [joints != null ? `${joints} joints` : null, scene.skin.skeleton, scene.skin.bind_pose]
      .filter((p): p is string => typeof p === 'string' && p !== '');
    if (parts.length) rows.push(['Skin', parts.join(' · ')]);
  }
  if (Array.isArray(scene.parts) && scene.parts.length) rows.push(['Parts', String(scene.parts.length)]);
  const extensions = Array.isArray(scene.extensions_used) ? scene.extensions_used.filter(e => typeof e === 'string') : [];
  if (extensions.length) rows.push(['Extensions', extensions.join(', ')]);
  return rows;
}

/** The triangles litegen counted in a delivered mesh, from its manifest. */
export function sceneTriangles(scene: unknown): number | null {
  return isObj(scene) ? finite(scene.triangle_count) : null;
}

/** The mesh's size in metres, from its manifest's bounds (the manifest is
 *  always in metres). Undefined without them: the viewer measures instead. */
export function sceneDimensions(scene: unknown): Dimensions | undefined {
  if (!isObj(scene) || !isObj(scene.bounds)) return undefined;
  const min = nums(scene.bounds.min);
  const max = nums(scene.bounds.max);
  if (min.length !== 3 || max.length !== 3) return undefined;
  return { x: max[0] - min[0], y: max[1] - min[1], z: max[2] - min[2] };
}

/** What delivery could not keep or do for the file (`scene.notes`). */
export function sceneNotes(scene: unknown): string[] {
  if (!isObj(scene) || !Array.isArray(scene.notes)) return [];
  return scene.notes.filter((n): n is string => typeof n === 'string' && n !== '');
}

/** The animation clips a delivered mesh carries, one row each, in the shared
 *  inspector's ClipTable shape. */
export function clipRows(scene: unknown): ClipRow[] {
  if (!isObj(scene) || !Array.isArray(scene.animations)) return [];
  return scene.animations.filter(isObj).map(a => {
    const duration = finite(a.duration_s);
    const travel = nums(a.root_travel_m);
    return {
      name: typeof a.name === 'string' ? a.name : '—',
      duration: duration != null ? `${duration.toFixed(2)} s` : '—',
      loop: a.loop_requested === true ? 'yes' : a.loop_requested === false ? 'no' : '—',
      inPlace: a.in_place === true ? (typeof a.in_place_by === 'string' && a.in_place_by ? `yes (${a.in_place_by})` : 'yes') : 'no',
      travel: travel.length ? `${Math.hypot(...travel).toFixed(2)} m` : '—',
    };
  });
}

/** The zip packages (`output_packages: true`): one per container whose mesh
 *  has files to go beside it (an OBJ's `.mtl` and textures, a DAE's or USDA's
 *  images), in the order the API listed them, which is request order. The
 *  dashboard only links them. Nothing here opens a zip. */
export function packageDownloads(assets: Model3dAsset[]): Model3dAsset[] {
  return assets.filter(a => a.kind === 'package');
}

/** The link text for a package: "Download OBJ package (.zip)". */
export function packageLabel(asset: Model3dAsset): string {
  return `Download ${asset.format.toUpperCase()} package (.zip)`;
}

/** The vendor's own file, delivered beside litegen's normalised mesh, if any. */
export function originalOf(assets: Model3dAsset[]): Model3dAsset | null {
  return assets.find(a => a.kind === 'original') ?? null;
}
