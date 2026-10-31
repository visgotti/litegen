import { describe, it, expect, vi, afterEach } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  LiteGenClient,
  LiteGenAPIError,
  LiteGenTimeoutError,
  LiteGenPollingTimeoutError,
  readResponseMeta,
  pollJob,
  VERSION,
  DEFAULT_TIMEOUT_MS,
  JOB_DEFAULT_TIMEOUT_MS,
  MODEL3D_DEFAULT_TIMEOUT_MS,
  type LiteGenResponseInfo,
} from "../src";

const here = (rel: string) => fileURLToPath(new URL(rel, import.meta.url));

describe("versioning (L19)", () => {
  it("names one semantic version in package.json, the code and the changelog's latest entry", () => {
    const pkg = JSON.parse(readFileSync(here("../package.json"), "utf8")) as {
      version: string;
      files: string[];
      scripts: Record<string, string>;
    };
    expect(pkg.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(VERSION).toBe(pkg.version);
    const changelog = readFileSync(here("../CHANGELOG.md"), "utf8");
    expect(/^## \[(\d+\.\d+\.\d+)\]/m.exec(changelog)?.[1]).toBe(pkg.version);
    expect(pkg.files).toContain("CHANGELOG.md");
    // A publish tests and builds the source it ships, never a stale dist/.
    expect(pkg.scripts.prepublishOnly).toBe("npm run typecheck && npm test && npm run build");
  });
});

describe("response headers (L19)", () => {
  const sent = {
    "content-type": "application/json",
    "x-litegen-dropped-params": "seed, negative_prompt",
    "x-ratelimit-limit": "60",
    "x-ratelimit-remaining": "59",
    "x-ratelimit-reset": "1793404800",
    "idempotent-replayed": "true",
  };

  it("reads what litegen's headers say", () => {
    expect(readResponseMeta(new Headers(sent))).toEqual({
      droppedParams: ["seed", "negative_prompt"],
      rateLimit: { limit: 60, remaining: 59, resetAt: new Date(1793404800 * 1000) },
      idempotentReplayed: true,
      retryAfterMs: undefined,
    });
    expect(readResponseMeta(new Headers())).toEqual({
      droppedParams: [],
      rateLimit: undefined,
      idempotentReplayed: false,
      retryAfterMs: undefined,
    });
  });

  it("hands every response to onResponse, and an error's headers to the error", async () => {
    let n = 0;
    const fetchImpl = (async () =>
      n++ === 0
        ? new Response(JSON.stringify({ status: "ok" }), { status: 200, headers: sent })
        : new Response(JSON.stringify({ error: { message: "slow down", type: "rate_limit_exceeded", code: "rate_limit_exceeded" } }), {
            status: 429,
            headers: { "content-type": "application/json", "retry-after": "7", "x-ratelimit-limit": "60", "x-ratelimit-remaining": "0" },
          })) as unknown as typeof fetch;
    const seen: LiteGenResponseInfo[] = [];
    const client = new LiteGenClient({ apiKey: "k", fetch: fetchImpl, onResponse: (r) => seen.push(r) });
    await client.health.live();
    const err = await client.health.live().catch((e: unknown) => e);
    expect(seen.map((r) => [r.method, r.path, r.status])).toEqual([
      ["GET", "/health/live", 200],
      ["GET", "/health/live", 429],
    ]);
    expect(seen[0]?.meta.droppedParams).toEqual(["seed", "negative_prompt"]);
    expect(seen[1]?.meta.rateLimit?.remaining).toBe(0);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    expect((err as LiteGenAPIError).headers?.get("x-ratelimit-remaining")).toBe("0");
    expect((err as LiteGenAPIError).retryAfterMs).toBe(7000);
  });
});

describe("timeouts (L19)", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("waits out the server's worst case — three 120 s vendor attempts — before giving up on a request", async () => {
    expect(DEFAULT_TIMEOUT_MS).toBeGreaterThan(3 * 120_000);
    vi.useFakeTimers();
    const hanging = ((_: unknown, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      })) as unknown as typeof fetch;
    const client = new LiteGenClient({ fetch: hanging });
    let settled: unknown = "pending";
    const call = client.health.live().then(
      () => "answered",
      (e: unknown) => e,
    );
    void call.then((v) => {
      settled = v;
    });
    await vi.advanceTimersByTimeAsync(3 * 120_000);
    expect(settled).toBe("pending");
    await vi.advanceTimersByTimeAsync(DEFAULT_TIMEOUT_MS - 3 * 120_000 + 1);
    expect(await call).toBeInstanceOf(LiteGenTimeoutError);
  });

  it("polls a job until the server must have decided it: it reaps one still running after 2 hours", async () => {
    expect(JOB_DEFAULT_TIMEOUT_MS).toBeGreaterThan(2 * 60 * 60_000);
    expect(MODEL3D_DEFAULT_TIMEOUT_MS).toBe(JOB_DEFAULT_TIMEOUT_MS);
    vi.useFakeTimers();
    const running = async () => ({ status: "processing" });
    const poll = (async () => {
      for await (const _ of pollJob("litegen-vid-1", running, { intervalMs: 60_000 })) {
        // keep polling
      }
    })().then(
      () => "done",
      (e: unknown) => e,
    );
    let settled: unknown = "pending";
    void poll.then((v) => {
      settled = v;
    });
    await vi.advanceTimersByTimeAsync(2 * 60 * 60_000);
    expect(settled).toBe("pending");
    await vi.advanceTimersByTimeAsync(JOB_DEFAULT_TIMEOUT_MS);
    expect(await poll).toBeInstanceOf(LiteGenPollingTimeoutError);
  });
});
