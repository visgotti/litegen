import { describe, it, expect, vi } from "vitest";
import {
  pollVideo,
  poll3d,
  pollJob,
  waitForCompletion,
  waitForModel3d,
  MODEL3D_DEFAULT_TIMEOUT_MS,
} from "../src/polling";
import { LiteGenAPIError, LiteGenPollingTimeoutError, LiteGenTimeoutError } from "../src/errors";
import { parseRetryAfter } from "../src/client";

type Update = { status: string; progress: number };

/** A getStatus stub that walks a fixed sequence of updates. */
function fakeStatus(seq: Update[]) {
  let i = 0;
  return async (_id: string) => {
    const item = seq[Math.min(i, seq.length - 1)];
    i += 1;
    return { id: "v1", model: "m", provider: "p", created: 0, ...item } as never;
  };
}

describe("pollVideo", () => {
  it("yields every update, including the terminal one", async () => {
    const getStatus = fakeStatus([
      { status: "processing", progress: 0 },
      { status: "processing", progress: 50 },
      { status: "completed", progress: 100 },
    ]);
    const progress: number[] = [];
    let lastStatus = "";
    for await (const u of pollVideo("v1", getStatus, { intervalMs: 0 })) {
      progress.push(u.progress as number);
      lastStatus = u.status as string;
    }
    expect(progress).toEqual([0, 50, 100]);
    expect(lastStatus).toBe("completed");
  });

  it("stops on a failed status", async () => {
    const getStatus = fakeStatus([
      { status: "processing", progress: 30 },
      { status: "failed", progress: 30 },
    ]);
    const seen: string[] = [];
    for await (const u of pollVideo("v1", getStatus, { intervalMs: 0 })) {
      seen.push(u.status as string);
    }
    expect(seen).toEqual(["processing", "failed"]);
  });
});

describe("LiteGenPollingTimeoutError names the family that timed out", () => {
  const neverTerminal = fakeStatus([{ status: "processing", progress: 10 }]);
  /** Drain a generator that is expected to throw before its first yield. */
  const drain = async (gen: AsyncGenerator<unknown, unknown, void>) => {
    for await (const update of gen) {
      void update; // unreachable: the deadline is already past
    }
  };

  it("says 'video' for a video poll", async () => {
    const err = await drain(pollVideo("v1", neverTerminal, { timeoutMs: -1 })).catch((e) => e);
    expect(err).toBeInstanceOf(LiteGenPollingTimeoutError);
    expect((err as LiteGenPollingTimeoutError).kind).toBe("video");
    expect((err as Error).message).toContain("Polling for video 'v1' timed out");
  });

  it("does NOT say 'video' for a 3D poll", async () => {
    const err = await drain(poll3d("litegen-3d-1", neverTerminal, { timeoutMs: -1 })).catch(
      (e) => e,
    );
    expect(err).toBeInstanceOf(LiteGenPollingTimeoutError);
    expect((err as LiteGenPollingTimeoutError).kind).toBe("3D generation");
    expect((err as Error).message).toBe("Polling for 3D generation 'litegen-3d-1' timed out");
    expect((err as Error).message).not.toContain("video");
  });

  it("falls back to the neutral 'job' when the family is unknown", async () => {
    const err = await drain(pollJob("x1", neverTerminal, { timeoutMs: -1 })).catch((e) => e);
    expect((err as LiteGenPollingTimeoutError).kind).toBe("job");
    expect((err as Error).message).toContain("Polling for job 'x1' timed out");
  });
});

describe("3D polling budget", () => {
  it("defaults to MODEL3D_DEFAULT_TIMEOUT_MS, not the 5-minute video default", async () => {
    vi.useFakeTimers();
    try {
      const getStatus = fakeStatus([{ status: "processing", progress: 10 }]);
      const settled = waitForModel3d("litegen-3d-1", getStatus, { intervalMs: 60_000 }).then(
        () => "resolved",
        (e: unknown) => e,
      );

      // Well past 5 minutes: a 3D job must still be polling.
      await vi.advanceTimersByTimeAsync(10 * 60_000);
      expect(await Promise.race([settled, Promise.resolve("still polling")])).toBe("still polling");

      // Past 30 minutes: now it gives up, naming the family.
      await vi.advanceTimersByTimeAsync(MODEL3D_DEFAULT_TIMEOUT_MS);
      const err = await settled;
      expect(err).toBeInstanceOf(LiteGenPollingTimeoutError);
      expect((err as LiteGenPollingTimeoutError).kind).toBe("3D generation");
    } finally {
      vi.useRealTimers();
    }
  });

  it("lets an explicit timeout win over the default", async () => {
    const getStatus = fakeStatus([{ status: "processing", progress: 10 }]);
    const err = await waitForModel3d("litegen-3d-1", getStatus, { timeoutMs: -1 }).catch((e) => e);
    expect(err).toBeInstanceOf(LiteGenPollingTimeoutError);
  });

  it("is 30 minutes", () => {
    expect(MODEL3D_DEFAULT_TIMEOUT_MS).toBe(30 * 60_000);
  });
});

describe("waitForCompletion", () => {
  it("returns the terminal state", async () => {
    const getStatus = fakeStatus([
      { status: "processing", progress: 20 },
      { status: "completed", progress: 100 },
    ]);
    const final = await waitForCompletion("v1", getStatus, { intervalMs: 0 });
    expect(final.status).toBe("completed");
    expect(final.progress).toBe(100);
  });
});

describe("transient status-read failures", () => {
  const apiError = (status: number, retryAfterMs?: number) =>
    new LiteGenAPIError(status, { message: `HTTP ${status}`, type: "http_error" }, retryAfterMs);
  // What a 30-minute 3D wait is likely to meet; throwing any of them would
  // abandon a job that keeps running (and billing) on the server.
  const transient = () => [
    apiError(429, 0),
    apiError(503),
    new LiteGenTimeoutError(),
    new TypeError("fetch failed"),
  ];

  /** A getStatus that throws each error in `seq` and returns each status, in order. */
  function flaky(seq: Array<Error | { status: string }>) {
    const calls = { n: 0 };
    const getStatus = async (_id: string) => {
      const item = seq[Math.min(calls.n, seq.length - 1)]!;
      calls.n += 1;
      if (item instanceof Error) throw item;
      return item;
    };
    return { getStatus, calls };
  }

  it("rides out a rate limit, a 503, a timeout and a dropped connection", async () => {
    const seq = [...transient(), { status: "processing" }, ...transient(), { status: "completed" }];
    const { getStatus, calls } = flaky(seq);
    const seen: string[] = [];
    for await (const u of pollJob("j", getStatus, { intervalMs: 0, timeoutMs: 5000 })) seen.push(u.status);
    expect(seen).toEqual(["processing", "completed"]);
    expect(calls.n).toBe(seq.length);
  });

  it.each([400, 401, 403, 404, 409])("throws a %i at once", async (status) => {
    const { getStatus, calls } = flaky([apiError(status)]);
    await expect(waitForModel3d("j", getStatus as never, { intervalMs: 0 })).rejects.toMatchObject({ status });
    expect(calls.n).toBe(1);
  });

  it("does not retry an abort or a programming error", async () => {
    for (const err of [new DOMException("aborted", "AbortError"), new TypeError("x is not a function")]) {
      const { getStatus, calls } = flaky([err]);
      await expect(waitForModel3d("j", getStatus as never, { intervalMs: 0 })).rejects.toBe(err);
      expect(calls.n).toBe(1);
    }
  });

  it("gives up after five failures in a row", async () => {
    const { getStatus, calls } = flaky([apiError(502)]);
    await expect(waitForModel3d("j", getStatus as never, { intervalMs: 0 })).rejects.toMatchObject({ status: 502 });
    expect(calls.n).toBe(6); // the first read and five retries
  });

  it("waits out Retry-After only until its deadline", async () => {
    const { getStatus } = flaky([{ status: "processing" }, apiError(429, 30_000)]);
    const started = Date.now();
    const err = await waitForModel3d("j", getStatus as never, { intervalMs: 0, timeoutMs: 200 }).catch((e) => e);
    expect(err).toBeInstanceOf(LiteGenPollingTimeoutError);
    expect((err as LiteGenPollingTimeoutError).lastStatus).toBe("processing");
    expect(Date.now() - started).toBeLessThan(2000);
  });

  it("reads Retry-After as seconds or an HTTP date", () => {
    expect(parseRetryAfter("2")).toBe(2000);
    expect(parseRetryAfter("-3")).toBe(0);
    expect(parseRetryAfter("Wed, 21 Oct 2015 07:28:00 GMT")).toBe(0);
    const inAMinute = parseRetryAfter(new Date(Date.now() + 60_000).toUTCString()) ?? 0;
    expect(inAMinute).toBeGreaterThan(50_000);
    expect(inAMinute).toBeLessThanOrEqual(60_000);
    expect(parseRetryAfter(null)).toBeUndefined();
    expect(parseRetryAfter(" ")).toBeUndefined();
    expect(parseRetryAfter("soon")).toBeUndefined();
  });
});
