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
 * Consecutive 404s tolerated before giving up (~60 s at 3 s). The spawned
 * backend task awaits `insert_generation` before this artifact is ever
 * visible (see `generate_video` / `generate_3d` in `handlers/mod.rs`), so by
 * the time the client can poll, the row should already exist; the retry
 * budget is a safety margin for scheduling jitter, not evidence of a race. A
 * 404 that persists past the budget means the insert failed (it is wrapped
 * in `let _ =` and its error discarded) or the row is in another org, and
 * polling it forever would hide that behind a spinner.
 */
export const MAX_NOT_FOUND_POLLS = 20;

const TERMINAL = new Set(['completed', 'failed', 'cancelled']);
const IN_FLIGHT = new Set(['pending', 'processing']);

/**
 * Consecutive transient errors (5xx, a request timeout/rate limit, or no
 * HTTP status at all — a network error) tolerated before giving up on a
 * minutes-long 3D/video poll. Unlike a 404, these say nothing about whether
 * the generation row exists — just that one request in a long-running poll
 * loop hiccupped — so the caller keeps showing the last known generation
 * with a "reconnecting…" note instead of discarding it.
 */
export const MAX_TRANSIENT_ERROR_STREAK = 3;

export function isTerminalStatus(status: string): boolean {
  return TERMINAL.has(status);
}

/** The outcome of one `generations.get` attempt. */
export type PollResult =
  | { kind: 'generation'; status: string }
  | { kind: 'error'; httpStatus?: number };

/**
 * Whether an HTTP status observed while polling is worth retrying quietly: a
 * 5xx, a request timeout (408) or rate limit (429), or no HTTP status at all
 * (a network error / a request that never got a response). A 404 is
 * deliberately excluded — it has its own bounded retry via
 * `MAX_NOT_FOUND_POLLS` and a distinct meaning (see above). Any other 4xx is
 * a real client-side problem and is never retried.
 */
export function isTransientError(httpStatus: number | undefined): boolean {
  if (httpStatus === undefined) return true;
  if (httpStatus === 408 || httpStatus === 429) return true;
  return httpStatus >= 500 && httpStatus < 600;
}

/**
 * The transient-error streak to carry into the next poll. Increments on a
 * transient error; resets to 0 on a successful poll or on any error that
 * isn't transient (a 404 keeps its own separate streak via `notFoundStreak`,
 * and a non-transient error stops polling outright, so its reset here is
 * moot but keeps the counter honest for a caller that inspects it anyway).
 */
export function nextTransientStreak(result: PollResult, prevStreak: number): number {
  if (result.kind === 'error' && isTransientError(result.httpStatus)) return prevStreak + 1;
  return 0;
}

/**
 * Whether to schedule another poll after `result`. `notFoundStreak` is the
 * number of consecutive 404s, including this one; `transientStreak` is the
 * number of consecutive transient errors (see `isTransientError`), including
 * this one — typically produced by `nextTransientStreak`.
 *
 * Only a known in-flight status keeps polling: an unrecognised status stops,
 * so a status added server-side later cannot turn into an unbounded poll
 * loop. A 404 keeps polling within its own budget. A transient error keeps
 * polling within `MAX_TRANSIENT_ERROR_STREAK`. Every other error — a
 * non-transient 4xx, or a streak that has run out — is final and the caller
 * renders it.
 */
export function shouldKeepPolling(result: PollResult, notFoundStreak = 0, transientStreak = 0): boolean {
  if (result.kind === 'generation') return IN_FLIGHT.has(result.status);
  if (result.httpStatus === 404) return notFoundStreak < MAX_NOT_FOUND_POLLS;
  return isTransientError(result.httpStatus) && transientStreak < MAX_TRANSIENT_ERROR_STREAK;
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
