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

  it("estimateCost posts to /v1/models3d/cost", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/models3d/cost");
      return jsonResponse({ base_cost_usd: 0.1, markup_usd: 0, total_cost_usd: 0.1, tokens_required: 100, cost_source: "estimated" });
    });
    const c = clientWith(fetchMock as unknown as typeof fetch);
    const est = await c.models3d.estimateCost({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    expect(est.total_cost_usd).toBe(0.1);
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
