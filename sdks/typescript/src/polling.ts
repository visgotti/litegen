import type { components } from "./generated/schema";
import { LiteGenPollingTimeoutError } from "./errors";

type VideoResponse = components["schemas"]["VideoGenerationResponse"];
type Model3dResponse = components["schemas"]["Model3dGenerationResponse"];

/** Any pollable job response. */
type WithStatus = { status: string };

export interface WaitForCompletionOptions {
  /** Milliseconds between polls. Default 2000. */
  intervalMs?: number;
  /** Total timeout in milliseconds. Default 5 minutes. */
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
 */
export const poll3d = (
  id: string,
  getStatus: (id: string) => Promise<Model3dResponse>,
  opts?: WaitForCompletionOptions,
): AsyncGenerator<Model3dResponse, Model3dResponse, void> =>
  pollJob<Model3dResponse>(id, getStatus, opts, MODEL3D_KIND);

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
