/**
 * Pure helpers for 3D generation results. Kept out of the component files so
 * they are unit-testable without a DOM and so the component modules export
 * only components (react-refresh requires that for hot reload).
 */
import type { Generation, Model3dAsset } from '@litegen/sdk';

const KINDS = new Set<Model3dAsset['kind']>(['mesh', 'texture', 'preview']);

/** `metadata` is arbitrary JSON on the wire, so every entry is checked before
 *  the renderer dereferences it — one malformed row must not blank the page. */
function isAsset(v: unknown): v is Model3dAsset {
  if (typeof v !== 'object' || v === null) return false;
  const a = v as Record<string, unknown>;
  return typeof a.url === 'string' && a.url !== '' && KINDS.has(a.kind as Model3dAsset['kind']);
}

/** Lowercased file extension of a URL's path ('' when it has none). */
function extensionOf(url: string): string {
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
  const listed = assets.find(a => a.kind === 'mesh');
  if (listed) return listed;
  if (!fallbackUrl) return null;
  return { kind: 'mesh', url: fallbackUrl, format: extensionOf(fallbackUrl) };
}

/** A generation row's assets (`metadata.assets`), with the mesh backfilled
 *  from `result_url` for older 3D rows. Never throws on malformed metadata. */
export function assetsOf(g: Generation): Model3dAsset[] {
  const raw = (g.metadata as { assets?: unknown } | null | undefined)?.assets;
  const assets = Array.isArray(raw) ? raw.filter(isAsset) : [];
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

/** User-facing explanation for a `<model-viewer>` `error` event's detail. */
export function describeViewerError(detail: unknown): string {
  const type = (detail as { type?: unknown } | null | undefined)?.type;
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
