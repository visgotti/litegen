import { describe, it, expect } from "vitest";
import { pollVideo, waitForCompletion } from "../src/polling";

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
