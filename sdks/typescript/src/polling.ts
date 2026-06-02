import type { components } from "./generated/schema";
import { LiteGenPollingTimeoutError } from "./errors";

type VideoResponse = components["schemas"]["VideoGenerationResponse"];

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
 * Poll a video job, yielding every status update — including the terminal one —
 * as an async iterable. Drive it with `for await`:
 *
 * ```ts
 * for await (const update of client.videos.poll(job.id)) {
 *   console.log(`${update.status} — ${update.progress}%`);
 * }
 * ```
 *
 * `progress` is LiteGen's unified 0–100 value; providers that don't report
 * fine-grained progress simply step toward 100, so this loop behaves the same
 * across every provider. Iteration stops after the first terminal status
 * (`completed` / `failed` / `cancelled`), which is the final value yielded.
 */
export async function* pollVideo(
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts: WaitForCompletionOptions = {},
): AsyncGenerator<VideoResponse, VideoResponse, void> {
  const intervalMs = opts.intervalMs ?? 2000;
  const timeoutMs = opts.timeoutMs ?? 5 * 60_000;
  const deadline = Date.now() + timeoutMs;

  let last: VideoResponse | undefined;
  while (true) {
    if (opts.signal?.aborted) {
      throw new DOMException("Polling aborted", "AbortError");
    }
    if (Date.now() > deadline) {
      throw new LiteGenPollingTimeoutError(id, last?.status as string | undefined);
    }
    last = await getStatus(id);
    yield last;
    if (TERMINAL_STATUSES.has(last.status as string)) {
      return last;
    }
    await sleep(intervalMs, opts.signal);
  }
}

/** Resolve once the video reaches a terminal status, returning the final state. */
export async function waitForCompletion(
  id: string,
  getStatus: (id: string) => Promise<VideoResponse>,
  opts: WaitForCompletionOptions = {},
): Promise<VideoResponse> {
  let last: VideoResponse | undefined;
  for await (const update of pollVideo(id, getStatus, opts)) {
    last = update;
  }
  // pollVideo always yields at least once before completing.
  return last as VideoResponse;
}

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
