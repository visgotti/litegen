import { describe, expect, it, vi } from "vitest";
import { LiteGenClient } from "../src/client";
import { pollJob } from "../src/polling";

type Resp = { id: string; status: string; progress: number; assets?: Array<{ kind: string; url: string }> };

function clientWith(fetchImpl: typeof fetch) {
  return new LiteGenClient({ baseUrl: "https://litegen.test", apiKey: "sk_test", fetch: fetchImpl });
}

function jsonResponse(body: unknown) {
  return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
}

describe("client.models3d", () => {
  it("submits to /v1/models3d/generations", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/models3d/generations");
      return jsonResponse({ id: "litegen-3d-1", status: "pending", progress: 0 });
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);
    const job = c.models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    const submitted = await job.submitted;
    expect(submitted.id).toBe("litegen-3d-1");
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("polls GET /v1/models3d/{id} until terminal and returns the mesh", async () => {
    const states: Resp[] = [
      { id: "litegen-3d-1", status: "pending", progress: 0 },
      { id: "litegen-3d-1", status: "processing", progress: 66 },
      {
        id: "litegen-3d-1",
        status: "completed",
        progress: 100,
        assets: [{ kind: "mesh", url: "https://cdn.test/litegen/3d/litegen-3d-1/model.glb" }],
      },
    ];
    let n = 0;
    const fetchMock = vi.fn(async (url: string) => {
      if (String(url).endsWith("/generations")) return jsonResponse(states[0]);
      expect(String(url)).toContain("/v1/models3d/litegen-3d-1");
      return jsonResponse(states[Math.min(++n, states.length - 1)]);
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);

    const final = await c.models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never, { intervalMs: 1 });
    expect(final.status).toBe("completed");
    expect(final.assets?.find((a) => a.kind === "mesh")?.url).toMatch(/^https:\/\/.*\.glb$/);
  });

  it("streams progress updates via for-await", async () => {
    const states: Resp[] = [
      { id: "j", status: "processing", progress: 33 },
      { id: "j", status: "completed", progress: 100, assets: [{ kind: "mesh", url: "https://cdn.test/m.glb" }] },
    ];
    let n = -1;
    const c = clientWith((async () => jsonResponse(states[Math.min(++n, 1)])) as unknown as typeof fetch);

    const seen: number[] = [];
    for await (const u of c.models3d.poll("j", { intervalMs: 1 })) seen.push(u.progress);
    expect(seen).toEqual([33, 100]);
  });

  it("retries a rate-limited status read after its Retry-After", async () => {
    // The per-key RPM limit covers status reads too; a 429 mid-wait must not
    // abandon a job that is still running.
    const answers = [
      new Response(JSON.stringify({ error: { message: "Rate limit exceeded", type: "rate_limit_exceeded", code: 429 } }), {
        status: 429,
        headers: { "content-type": "application/json", "retry-after": "0" },
      }),
      new Response("", { status: 503, headers: { "retry-after": "0" } }),
      jsonResponse({ id: "j", status: "completed", progress: 100, assets: [{ kind: "mesh", url: "https://cdn.test/m.glb" }] }),
    ];
    let reads = 0;
    const fetchMock = vi.fn(async (url: string) => {
      if (String(url).endsWith("/generations")) return jsonResponse({ id: "j", status: "pending", progress: 0 });
      return answers[reads++];
    });
    const final = await clientWith(fetchMock as unknown as typeof fetch).models3d.generate(
      { model: "mock/mesh-3d", prompt: "a fox" } as never,
      { intervalMs: 0 },
    );
    expect(final.status).toBe("completed");
    expect(reads).toBe(3);
  });

  it("estimateCost posts to /v1/models3d/cost", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/models3d/cost");
      return jsonResponse({ base_cost_usd: 0.1, markup_usd: 0, total_cost_usd: 0.1, tokens_required: 100, cost_source: "estimated" });
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);
    const est = await c.models3d.estimateCost({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    expect(est.total_cost_usd).toBe(0.1);
  });

  // An abort listener added to an already-aborted signal never fires, so these
  // requests used to go out anyway: a cancelled run could still start a job.
  it("sends nothing when the caller's signal is already aborted", async () => {
    const fetchMock = vi.fn(async () => jsonResponse({ id: "j", status: "pending", progress: 0 }));
    const ctrl = new AbortController();
    ctrl.abort();
    const job = clientWith(fetchMock as unknown as typeof fetch).models3d.generate(
      { model: "mock/mesh-3d", prompt: "a fox" } as never,
      { signal: ctrl.signal },
    );
    await expect(job.submitted).rejects.toMatchObject({ name: "AbortError" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("sends nothing when the signal aborts while the CSRF token is fetched", async () => {
    const fetchMock = vi.fn(async () => jsonResponse({ id: "j", status: "pending", progress: 0 }));
    const ctrl = new AbortController();
    const c = new LiteGenClient({
      baseUrl: "https://litegen.test",
      fetch: fetchMock as unknown as typeof fetch,
      getCsrfToken: async () => {
        ctrl.abort();
        return "csrf";
      },
    });
    const job = c.models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never, { signal: ctrl.signal });
    await expect(job.submitted).rejects.toMatchObject({ name: "AbortError" });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("client.models.list", () => {
  it("narrows the catalog with ?media_type=", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toBe("https://litegen.test/v1/models?media_type=model3d");
      return jsonResponse({ data: [{ id: "mock/mesh-3d" }] });
    });
    const models = await clientWith(fetchMock as unknown as typeof fetch).models.list({
      media_type: "model3d",
    });
    expect(models.map((m) => m.id)).toEqual(["mock/mesh-3d"]);
  });

  it("sends no query when unfiltered", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toBe("https://litegen.test/v1/models");
      return jsonResponse({ data: [] });
    });
    await clientWith(fetchMock as unknown as typeof fetch).models.list();
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("still accepts the original list(signal) form", async () => {
    const ctrl = new AbortController();
    const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
      expect(String(url)).toBe("https://litegen.test/v1/models");
      // The client wraps the caller's signal in its own timeout controller;
      // aborting the caller's must abort the request.
      ctrl.abort();
      expect(init?.signal?.aborted).toBe(true);
      return jsonResponse({ data: [] });
    });
    await clientWith(fetchMock as unknown as typeof fetch).models.list(ctrl.signal);
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});

describe("pollJob", () => {
  it("is generic over any { status } response", async () => {
    // The point of the refactor: poll3d is a two-line wrapper, not a copy.
    const states = [{ status: "processing" }, { status: "completed" }];
    let n = -1;
    const seen: string[] = [];
    for await (const u of pollJob("x", async () => states[Math.min(++n, 1)]!, { intervalMs: 1 })) {
      seen.push(u.status);
    }
    expect(seen).toEqual(["processing", "completed"]);
  });
});
