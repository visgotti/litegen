import { describe, it, expect } from "vitest";
import { pollVideo, poll3d, pollJob, waitForCompletion } from "../src/polling";
import { LiteGenPollingTimeoutError } from "../src/errors";

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
