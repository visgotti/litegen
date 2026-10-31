/**
 * Pure helpers for 3D generation results. Kept out of the component files so
 * they are unit-testable without a DOM and so the component modules export
 * only components (react-refresh requires that for hot reload).
 */
import type { Generation, Model3dAsset } from '@litegen/sdk';

const KINDS = new Set<Model3dAsset['kind']>(['mesh', 'texture', 'preview']);

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

const GLTF_FORMATS = new Set(['glb', 'gltf']);

/** Whether `<model-viewer>` can render a mesh of this format. It is glTF-only,
 *  so an OBJ/FBX/USDZ mesh would be fetched in full and then fail as if it were
 *  corrupt. An empty (unknown) format is attempted and left to the viewer. */
export function canPreviewMesh(format: unknown): boolean {
  // Guarded, not trusted: callers forward `format` from unvalidated poll
  // responses, and a throw in render unmounts the page (no error boundary).
  if (typeof format !== 'string') return false;
  return format === '' || GLTF_FORMATS.has(format.toLowerCase());
}

/**
 * The URL to hand `<model-viewer>` on retry attempt `n` (0 = first try).
 * model-viewer 4.3.1 caches a FAILED load as an empty placeholder keyed by the
 * exact src string, with no public way to evict it, so retrying the same
 * string fails instantly. A new string forces a fresh load.
 *
 * A FRAGMENT, deliberately — do not "simplify" this to a query param:
 * fragments are never sent on the wire (three's FileLoader does
 * `fetch(new Request(url))`), so presigned / signed-CDN URLs stay valid, where
 * an extra query param would break their signature. It contains no '/', so
 * LoaderUtils.extractUrlBase (lastIndexOf('/')) still resolves a .gltf's
 * relative buffers against the real directory.
 */
export function retrySrc(src: string, n: number): string {
  return n > 0 ? `${src}#retry-${n}` : src;
}

/**
 * Whether a `<model-viewer>` `load` event belongs to the src now displayed.
 * When src changes mid-load, model-viewer cancels the old load silently but
 * still dispatches `load` with the OLD url in `detail.url` (4.3.1,
 * model-viewer-base.js $updateSource). A detail without a url is accepted, so
 * a version that stops sending it degrades to the old behaviour instead of
 * never settling.
 */
export function loadEventMatches(detail: unknown, src: string): boolean {
  const url = (detail as { url?: unknown } | null | undefined)?.url;
  return typeof url !== 'string' || url === src;
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
  // A generation can now carry one mesh per requested container, and the viewer
  // renders glTF only — so taking the first would show "unsupported format" for
  // a generation that did deliver a perfectly good glb, purely because the
  // adapter happened to push the obj first. glb first, exactly like
  // `Model3dGenerationResponse::mesh()` on the backend (which picks the same
  // asset for `result_url`); then any other glTF — a `gltf` can arrive
  // alongside the glb now that litegen converts to it — then whatever is first.
  const listed = meshes.find(a => a.format.toLowerCase() === 'glb')
    ?? meshes.find(a => canPreviewMesh(a.format))
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

const BYTE_UNITS = ['KB', 'MB', 'GB', 'TB'];

/** Human file size in binary units. A dash for unknown or nonsensical input. */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n < 0) return '—';
  if (n < 1024) return `${Math.round(n)} B`;
  let value = n / 1024;
  let unit = 0;
  // Compare the *rounded* value so 1,048,575 B reads "1.0 MB", not "1024.0 KB".
  while (Number(value.toFixed(1)) >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${BYTE_UNITS[unit]}`;
}

export interface Dimensions { x: number; y: number; z: number }

function formatLength(n: number): string {
  // glTF units are metres; a model authored in millimetres would read as
  // "0.00" at fixed precision, so small non-zero values switch to exponent form.
  return n === 0 || Math.abs(n) >= 0.01 ? n.toFixed(2) : n.toExponential(1);
}

/** Bounding-box size as `W × H × D m` (model-viewer reports metres). */
export function formatDimensions(d: Dimensions | null | undefined): string {
  if (!d || ![d.x, d.y, d.z].every(Number.isFinite)) return '—';
  return `${formatLength(d.x)} × ${formatLength(d.y)} × ${formatLength(d.z)} m`;
}

const IMAGE_FORMATS = new Set(['png', 'jpg', 'jpeg', 'webp', 'gif', 'avif']);

/** Whether an asset of this format can be shown in a plain `<img>` (a KTX2
 *  or EXR texture is downloadable but not previewable). */
export function isBrowserImage(format: string): boolean {
  return IMAGE_FORMATS.has(format.toLowerCase());
}

/** 'load': a download/network/HTTP failure — a second attempt might succeed,
 *  so Retry is offered. 'parse': the bytes arrived but the file is not valid
 *  glTF — Retry re-fetches the exact same bytes and fails the same way, so it
 *  is withheld. */
export type ViewerErrorReason = 'load' | 'parse';

/**
 * Classifies a `<model-viewer>` `error` event as a download failure or a
 * parse failure.
 *
 * This cannot be done from `detail` alone. Verified against the installed
 * `@google/model-viewer` 4.3.1 in a real browser (see model3d-assets.test.ts
 * and the Task 17B report): `CachingGLTFLoader.preload()` swallows the real
 * cause of a failed load — an `HttpError` for a 404, a `RangeError` such as
 * "Invalid typed array length" for a corrupt GLB — logging it with
 * `console.error` and substituting an empty glTF instance. The `error` event
 * this component sees always carries the SAME generic
 * `sourceError: TypeError: Cannot read properties of undefined (reading
 * 'scene')`, thrown later when that empty instance is cloned, for both
 * failure modes and for every glTF-level parse error, not just this one.
 *
 * The one signal that does discriminate is outside `detail`:
 * `PerformanceResourceTiming.responseStatus` for the same request (Resource
 * Timing Level 2), passed in by the caller — reading it needs a live
 * `PerformanceObserver`, which does not belong in a pure helper (see
 * `ModelViewer.tsx` and `matchResourceStatus` below for how it's captured;
 * the global `performance.getEntriesByType('resource')` buffer is NOT used —
 * it's capped and this SPA's polling fills it within a few generations). It
 * reports whether the browser actually received a response for the mesh,
 * with no extra request of our own (so a presigned/signed mesh URL is never
 * fetched twice). A 2xx/3xx status means the bytes arrived, so a
 * `loadfailure` after that is a parse failure. Anything else — no entry, a
 * 4xx/5xx, or status 0 (three.js's `FileLoader` fetches in `cors` mode, so a
 * response that reached the app already passed CORS; a real cross-origin
 * mesh host reports its true status here, and 0 means the request never got
 * a response at all — not "no `Timing-Allow-Origin`", a header this does not
 * depend on) — is treated as a download failure so Retry stays available.
 * This also keeps `webglcontextlost` (a failure unrelated to the fetch,
 * which can occur long after a successful load) as `load` rather than
 * misreading a stale successful timing entry. Browsers without
 * `responseStatus` yet (e.g. Safari at this writing) always read as
 * "unknown" here too, so they always get the safe `load` classification.
 */
export function classifyViewerError(detail: unknown, responseStatus: number | null): ViewerErrorReason {
  const type = (detail as { type?: unknown } | null | undefined)?.type;
  if (type !== 'loadfailure') return 'load';
  return typeof responseStatus === 'number' && responseStatus >= 200 && responseStatus < 400 ? 'parse' : 'load';
}

/** The shape of a `PerformanceResourceTiming` entry this module reads.
 *  `responseStatus` is undefined in browsers that don't implement Resource
 *  Timing Level 2 yet (e.g. Safari as of this writing) — absence must read
 *  as "unknown" (`null`, via `matchResourceStatus`), not "0", so it is never
 *  mistaken for the real opaque/failed-request value. */
export interface ResourceStatusEntry { name: string; responseStatus?: number }

/**
 * Picks the HTTP status of the most recent entry in `entries` whose `name`
 * equals `resolvedSrc` exactly, or null when none matches or the match has
 * no usable status.
 *
 * Pure and DOM-free: `entries` is handed in by the caller, already read live
 * from a `PerformanceObserver` scoped to the current `<model-viewer>` — never
 * from the global `performance.getEntriesByType('resource')` buffer, which is
 * capped (250 entries by default) and gets filled by this SPA's own polling
 * (generation jobs, Overview, Logs) within a few generations, after which a
 * read of the global buffer would silently stop seeing new mesh requests at
 * all. Matching is on the FULL resolved URL, fragment included — Chromium
 * retains `retrySrc`'s `#retry-N` cache-buster in a resource-timing entry's
 * `name` (verified empirically, see the Task 17B report), so each retry
 * gets its own entry and cannot be confused with an earlier attempt WITHIN
 * the same mount.
 *
 * That is the only staleness the fragment rules out. Across mounts it rules
 * out nothing: a fresh viewer's first attempt uses the bare `src` again — the
 * same URL the previous mount's first attempt used. Freshness there is the
 * caller's job, and depends on the observer being created per load with
 * `buffered: false` so it only ever sees entries for the load in flight.
 * Reading a buffered observer (or the global buffer) here would bring back
 * the R47 bug: a real 404 matched against the previous mount's 200 and
 * reported as a parse failure, with Retry withheld.
 */
export function matchResourceStatus(entries: ResourceStatusEntry[], resolvedSrc: string): number | null {
  let status: number | null = null;
  for (const e of entries) {
    if (e.name === resolvedSrc) {
      status = typeof e.responseStatus === 'number' ? e.responseStatus : null;
    }
  }
  return status;
}

/** User-facing explanation for a `<model-viewer>` `error` event's detail.
 *  `reason` is `classifyViewerError`'s result for the same event, so the
 *  message matches whether Retry is offered. */
export function describeViewerError(detail: unknown, reason: ViewerErrorReason = 'load'): string {
  const type = (detail as { type?: unknown } | null | undefined)?.type;
  if (reason === 'parse') {
    return "The mesh file isn't a valid glTF.";
  }
  if (type === 'loadfailure') {
    return 'The mesh file could not be loaded — it may be missing (404) or corrupt.';
  }
  if (type === 'webglcontextlost') {
    return 'The browser lost its WebGL context while rendering this mesh.';
  }
  return 'The browser could not render this mesh.';
}

/** The slice of the Clipboard API this module uses. */
export interface ClipboardLike { writeText(text: string): Promise<void> }

/**
 * Copies text, resolving to whether it worked. Never throws: the Clipboard API
 * is absent outside secure contexts (plain-http deployments) and rejects when
 * permission is denied, and neither should surface as an uncaught error.
 * The clipboard is passed in rather than read from `navigator` so the missing
 * case is testable in node.
 */
export async function copyToClipboard(text: string, clipboard: ClipboardLike | null | undefined): Promise<boolean> {
  if (!clipboard || typeof clipboard.writeText !== 'function') return false;
  try {
    await clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
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

export interface ClipRow {
  name: string;
  duration: string;
  /** Whether the clip was asked to loop. */
  loop: string;
  /** Whether it plays in place, and who made it so. */
  inPlace: string;
  /** How far the root moves over the clip. */
  travel: string;
}

/** The animation clips a delivered mesh carries, one row each. */
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

/** The vendor's own file, delivered beside litegen's normalised mesh, if any. */
export function originalOf(assets: Model3dAsset[]): Model3dAsset | null {
  return assets.find(a => a.kind === 'original') ?? null;
}
