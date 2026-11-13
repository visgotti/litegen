// How both apps write a model's sizes and counts. Formerly two copies: the
// dashboard's model3d-assets.ts and the converter tool's format-helpers.ts.

export interface Dimensions {
  x: number;
  y: number;
  z: number;
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

function formatLength(n: number): string {
  // A model authored in millimetres would read "0.00" at fixed precision, so
  // small non-zero values switch to exponent form.
  return n === 0 || Math.abs(n) >= 0.01 ? n.toFixed(2) : n.toExponential(1);
}

/** Bounding-box size as `W × H × D m`. */
export function formatDimensions(d: Dimensions | null | undefined): string {
  if (!d || ![d.x, d.y, d.z].every(Number.isFinite)) return '—';
  return `${formatLength(d.x)} × ${formatLength(d.y)} × ${formatLength(d.z)} m`;
}

/** A note when the format's three.js loader drew fewer triangles than litegen
 *  reads in the same file: the loader skipped part of it (three's USDZLoader,
 *  for one, draws no referenced prim). More is not news: a mesh that several
 *  nodes place is drawn once per node and read once. */
export function drawnNote(read: number | null | undefined, drawn: number | null | undefined): string | null {
  if (read == null || drawn == null || drawn >= read) return null;
  return `three.js's loader for this format drew ${drawn.toLocaleString('en-US')} of the ${read.toLocaleString('en-US')} triangles litegen reads in this file.`;
}

/** The slice of the Clipboard API this module uses. */
export interface ClipboardLike {
  writeText(text: string): Promise<void>;
}

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
