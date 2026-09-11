/**
 * Pure decisions behind resolving an async request artifact to its generation
 * row (Logs → trace panel → Visual tab). Kept out of the component files so
 * they are unit-testable without a DOM and so the component modules export
 * only components (react-refresh requires that for hot reload).
 *
 * Why resolve at all: an async (video / model3d) artifact is written at submit
 * time, before the result exists, and is never updated. The generation row —
 * same id as the artifact's `request_id` — is the single source of truth for
 * status, progress, the result URL and the 3D asset list.
 */
import type { RequestArtifact } from '@litegen/sdk';
import { extensionOf, isBrowserImage } from './model3d-assets';

/** How often a non-terminal generation is re-fetched. */
export const POLL_INTERVAL_MS = 3000;

/**
 * Consecutive 404s tolerated before giving up (~60 s at 3 s). A 404 right
 * after submit is expected — the handler spawns the generation insert after
 * it has sent the HTTP response — so it is retried. A 404 that persists is
 * not "not persisted yet" any more (the insert failed, or the row is in
 * another org), and polling it forever would hide that behind a spinner.
 */
export const MAX_NOT_FOUND_POLLS = 20;

const TERMINAL = new Set(['completed', 'failed', 'cancelled']);
const IN_FLIGHT = new Set(['pending', 'processing']);

export function isTerminalStatus(status: string): boolean {
  return TERMINAL.has(status);
}

/** The outcome of one `generations.get` attempt. */
export type PollResult =
  | { kind: 'generation'; status: string }
  | { kind: 'error'; httpStatus?: number };

/**
 * Whether to schedule another poll after `result`. `notFoundStreak` is the
 * number of consecutive 404s, including this one.
 *
 * Only a known in-flight status keeps polling: an unrecognised status stops,
 * so a status added server-side later cannot turn into an unbounded poll loop.
 * Every error other than a 404 is final (the caller renders it).
 */
export function shouldKeepPolling(result: PollResult, notFoundStreak = 0): boolean {
  if (result.kind === 'generation') return IN_FLIGHT.has(result.status);
  return result.httpStatus === 404 && notFoundStreak < MAX_NOT_FOUND_POLLS;
}

/** The HTTP status carried by a thrown `LiteGenAPIError`, if any. Duck-typed
 *  rather than `instanceof`, which is fragile across the symlinked SDK copy. */
export function httpStatusOf(err: unknown): number | undefined {
  const status = (err as { status?: unknown } | null | undefined)?.status;
  return typeof status === 'number' ? status : undefined;
}

/**
 * Whether the Visual tab should render this artifact from its generation row
 * instead of from the artifact itself.
 *
 * - video: only while the artifact has no URL. A provider that answers
 *   synchronously already put the final URL on the artifact.
 * - model3d: always. Its output is an asset list (mesh + textures + preview)
 *   that exists only in the generation's `metadata.assets`; no path writes a
 *   3D artifact's `output_value`, and a single URL could not carry the list.
 */
export function resolvesViaGeneration(artifact: RequestArtifact): boolean {
  if (artifact.output_kind !== 'url') return false;
  if (artifact.media_type === 'model3d') return true;
  return artifact.media_type === 'video' && !artifact.output_value;
}

/**
 * Whether a video generation's result is actually an image (an animated GIF,
 * e.g. mock/visual-video-gen), which must go in an `<img>` to animate — the
 * same special case `VisualTab` makes from `output_mime`. A generation row
 * carries no mime, so the URL is checked too: a `data:image/` URL, or an
 * image file extension.
 */
export function videoResultIsImage(url: string | null | undefined, mime?: string | null): boolean {
  if (mime) return mime.startsWith('image/');
  if (!url) return false;
  if (url.startsWith('data:')) return url.startsWith('data:image/');
  return isBrowserImage(extensionOf(url));
}
