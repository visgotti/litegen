import type { components } from "./generated/schema";
import { LiteGenPollingTimeoutError } from "./errors";

type VideoResponse = components["schemas"]["VideoGenerationResponse"];
type Model3dResponse = components["schemas"]["Model3dGenerationResponse"];

/** Any pollable job response. */
type WithStatus = { status: string };

export interface WaitForCompletionOptions {
  /** Milliseconds between polls. Default 2000. */
  intervalMs?: number;
  /**
   * Total timeout in milliseconds. Default 5 minutes, except 3D jobs, which
   * default to {@link MODEL3D_DEFAULT_TIMEOUT_MS}.
   */
  timeoutMs?: number;
  /** Optional AbortSignal to cancel polling. */
  signal?: AbortSignal;
}

const TERMINAL_STATUSES = new Set<string>(["completed", "failed", "cancelled"]);

/**
 * What a timeout on this family calls itself. Exported so the 3D wrappers that
 * go through the generic `waitForJob` report the same word as `poll3d`.
 */
export const MODEL3D_KIND = "3D generation";

/**
 * Default polling budget for a 3D job: 30 minutes, not the 5 every other family
 * gets. A vendor 3D job can sit in the vendor's queue and then refine for well
 * over 5 minutes, and giving up client-side does not stop it — the server job
 * runs on to completion and is billed, so a short default throws away results
 * the caller paid for. 30 minutes stays inside the backend's 2-hour reap age,
 * past which the server fails the row itself.
 */
export const MODEL3D_DEFAULT_TIMEOUT_MS = 30 * 60_000;

/** `opts` with the 3D family's default timeout filled in where none was given. */
function withModel3dDefaults(opts: WaitForCompletionOptions = {}): WaitForCompletionOptions {
  return { ...opts, timeoutMs: opts.timeoutMs ?? MODEL3D_DEFAULT_TIMEOUT_MS };
}

/**
 * Poll any async job, yielding every status update — including the terminal one
 * — as an async iterable. `progress` is LiteGen's unified 0–100 value across
 * every provider and modality, so this loop behaves identically for video and
 * 3D — providers that don't report fine-grained progress simply step toward
 * 100. Iteration stops after the first terminal status (`completed` / `failed`
 * / `cancelled`), which is the final value yielded.
 *
 * `pollVideo` / `poll3d` are thin instantiations of this generator, not
 * separate implementations — a new async media family gets polling for free
 * by wrapping this with its own response type.
 */
export async function* pollJob<T extends WithStatus>(
  id: string,
  getStatus: (id: string) => Promise<T>,
  opts: WaitForCompletionOptions = {},
  kind = "job",
): AsyncGenerator<T, T, void> {
  const intervalMs = opts.intervalMs ?? 2000;
  const timeoutMs = opts.timeoutMs ?? 5 * 60_000;
  const deadline = Date.now() + timeoutMs;

  let last: T | undefined;
  while (true) {
    if (opts.signal?.aborted) {
      throw new DOMException("Polling aborted", "AbortError");
    }
    if (Date.now() > deadline) {
      throw new LiteGenPollingTimeoutError(id, last?.status, kind);
    }
    last = await getStatus(id);
    yield last;
    if (TERMINAL_STATUSES.has(last.status)) {
      return last;
    }
    await sleep(intervalMs, opts.signal);
  }
}

/** Resolve once any job reaches a terminal status, returning the final state. */
export async function waitForJob<T extends WithStatus>(
  id: string,
  getStatus: (id: string) => Promise<T>,
  opts: WaitForCompletionOptions = {},
  kind = "job",
): Promise<T> {
  let last: T | undefined;
  for await (const update of pollJob(id, getStatus, opts, kind)) {
    last = update;
  }
  // pollJob always yields at least once before completing.
  return last as T;
}

/**
 * Poll a video job. `for await (const update of client.videos.poll(job.id))`.
 */
export const pollVideo = (
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts?: WaitForCompletionOptions,
): AsyncGenerator<VideoResponse, VideoResponse, void> =>
  pollJob<VideoResponse>(id, getStatus, opts, "video");

/**
 * Poll a 3D generation job. `for await (const update of client.models3d.poll(job.id))`.
 * Defaults to {@link MODEL3D_DEFAULT_TIMEOUT_MS} when `opts.timeoutMs` is unset.
 */
export const poll3d = (
  id: string,
  getStatus: (id: string) => Promise<Model3dResponse>,
  opts?: WaitForCompletionOptions,
): AsyncGenerator<Model3dResponse, Model3dResponse, void> =>
  pollJob<Model3dResponse>(id, getStatus, withModel3dDefaults(opts), MODEL3D_KIND);

/**
 * Resolve once a 3D job reaches a terminal status, returning the final state.
 * Same default timeout as {@link poll3d}.
 */
export const waitForModel3d = (
  id: string,
  getStatus: (id: string) => Promise<Model3dResponse>,
  opts?: WaitForCompletionOptions,
): Promise<Model3dResponse> =>
  waitForJob<Model3dResponse>(id, getStatus, withModel3dDefaults(opts), MODEL3D_KIND);

/**
 * Back-compat wrapper. Deliberately concrete rather than a bare alias for
 * `waitForJob`: exporting the generic directly makes TypeScript infer `T` from
 * the caller's `getStatus`, which silently changes inference at existing call
 * sites that relied on the concrete `VideoResponse` signature.
 */
export const waitForCompletion = (
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts?: WaitForCompletionOptions,
): Promise<VideoResponse> => waitForJob<VideoResponse>(id, getStatus, opts, "video");

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => resolve(), ms);
    signal?.addEventListener(
      "abort",
      () => {
        clearTimeout(t);
        reject(new DOMException("Polling aborted", "AbortError"));
      },
      { once: true },
    );
  });
}
