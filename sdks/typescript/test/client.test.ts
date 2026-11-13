import { describe, it, expect, vi } from "vitest";
import {
  LiteGenClient,
  LiteGenAPIError,
  GenerationStatus,
  RefImageKind,
  isAccepted,
} from "../src";

interface CapturedRequest {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function mockFetch(
  handler: (req: CapturedRequest) => Response | Promise<Response>,
): typeof fetch {
  return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers: Record<string, string> = {};
    const h = new Headers(init?.headers);
    h.forEach((v, k) => {
      headers[k.toLowerCase()] = v;
    });
    const body = init?.body ? JSON.parse(init.body as string) : undefined;
    return handler({ url, method: init?.method ?? "GET", headers, body });
  }) as unknown as typeof fetch;
}

describe("LiteGenClient.images.submit (late and async images)", () => {
  it("asks not to wait and says the answer is the generation to come", async () => {
    const captured: Partial<CapturedRequest> = {};
    const fetchImpl = mockFetch(async (req) => {
      Object.assign(captured, req);
      return new Response(
        JSON.stringify({ id: "litegen-img-1", object: "image.generation", status: "processing", model: "bfl/flux-pro-1.1", created: 1 }),
        { status: 202, headers: { "Content-Type": "application/json", Location: "/v1/generations/litegen-img-1" } },
      );
    });
    const client = new LiteGenClient({ apiKey: "lg-test", fetch: fetchImpl });
    const req = { prompt: "a cat", model: "bfl/flux-pro-1.1", n: 1, strict: true, reference_images: [], response_format: "url" };
    const r = await client.images.submit(req, { waitSeconds: 5 });
    expect(captured.headers?.["prefer"]).toBe("respond-async, wait=5");
    expect(isAccepted(r)).toBe(true);
    if (isAccepted(r)) expect(r.id).toBe("litegen-img-1");
    await client.images.submit(req);
    expect(captured.headers?.["prefer"]).toBe("respond-async");
  });

  it("an image that came back at once is not an accepted job", () => {
    expect(isAccepted({ created: 1, data: [], model: "m", provider: "p", id: "img-1" } as never)).toBe(false);
  });

  it("waits for a late image through generations.waitForCompletion", async () => {
    let calls = 0;
    const fetchImpl = mockFetch(async (req) => {
      calls += 1;
      expect(req.url).toContain("/v1/generations/litegen-img-1");
      return new Response(JSON.stringify({ id: "litegen-img-1", status: calls < 2 ? "processing" : "completed" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });
    const client = new LiteGenClient({ apiKey: "lg-test", fetch: fetchImpl });
    const g = await client.generations.waitForCompletion("litegen-img-1", { intervalMs: 1 });
    expect(g.status).toBe("completed");
    expect(calls).toBe(2);
  });
});

describe("LiteGenClient.images.generate", () => {
  it("posts to /v1/images/generations with auth header and body", async () => {
    const captured: Partial<CapturedRequest> = {};
    const fetchImpl = mockFetch(async (req) => {
      Object.assign(captured, req);
      return new Response(
        JSON.stringify({
          created: 1,
          data: [{ url: "https://x.png", content_type: "image/png", index: 0 }],
          model: "openai/dall-e-3",
          provider: "openai",
          id: "img-1",
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    });
    const client = new LiteGenClient({ apiKey: "lg-test", fetch: fetchImpl });
    const resp = await client.images.generate({
      prompt: "a cat",
      model: "openai/dall-e-3",
      n: 1,
      strict: true,
      reference_images: [],
      response_format: "url",
    });
    expect(captured.url).toContain("/v1/images/generations");
    expect(captured.method).toBe("POST");
    expect(captured.headers?.["authorization"]).toBe("Bearer lg-test");
    expect((captured.body as { prompt: string }).prompt).toBe("a cat");
    expect(resp.id).toBe("img-1");
  });

  it("throws LiteGenAPIError on non-2xx with parsed error detail", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(
          JSON.stringify({
            error: { message: "bad prompt", type: "validation_error", code: "400" },
          }),
          { status: 400, headers: { "Content-Type": "application/json" } },
        ),
    );
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    await expect(
      client.images.generate({
        prompt: "",
        model: "x",
        n: 1,
        strict: true,
        reference_images: [],
        response_format: "url",
      }),
    ).rejects.toMatchObject({
      name: "LiteGenAPIError",
      status: 400,
      type: "validation_error",
      message: "bad prompt",
    });
  });
});

describe("vendor errors (L15)", () => {
  it("carries the vendor's own answer and whether to retry", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(
          JSON.stringify({
            error: {
              message: "fal failed for model fal/flux-dev: upstream overloaded",
              type: "vendor_error",
              code: "vendor_error",
              retryable: true,
              provider_error: { status: 503, code: null, message: "upstream overloaded" },
            },
          }),
          { status: 502, headers: { "Content-Type": "application/json" } },
        ),
    );
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const err = await client.images
      .generate({ prompt: "x", model: "fal/flux-dev", n: 1, strict: true, reference_images: [], response_format: "url" })
      .catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    const api = err as LiteGenAPIError;
    expect([api.code, api.retryable, api.providerError?.status]).toEqual(["vendor_error", true, 503]);
  });
});

describe("LiteGenClient.videos.waitForCompletion", () => {
  it("polls until status is completed", async () => {
    let calls = 0;
    const fetchImpl = mockFetch(async (req) => {
      if (req.url.endsWith("/v1/videos/vid-1")) {
        calls++;
        const status = calls < 3 ? "processing" : "completed";
        return new Response(
          JSON.stringify({
            id: "vid-1",
            status,
            model: "m",
            provider: "p",
            progress: status === "completed" ? 100 : 50,
            created: 1,
            video_url: status === "completed" ? "https://done.mp4" : null,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      return new Response("nope", { status: 404 });
    });
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const final = await client.videos.waitForCompletion("vid-1", {
      intervalMs: 10,
      timeoutMs: 2000,
    });
    expect(final.status).toBe(GenerationStatus.Completed);
    expect(calls).toBe(3);
  });
});

describe("LiteGenClient.videos.generate (VideoJob)", () => {
  function jobFetch() {
    let polls = 0;
    return mockFetch(async (req) => {
      if (req.method === "POST" && req.url.endsWith("/v1/videos/generations")) {
        return new Response(
          JSON.stringify({ id: "vid-9", status: "processing", model: "m", provider: "p", progress: 0, created: 1 }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      if (req.url.endsWith("/v1/videos/vid-9")) {
        polls++;
        const done = polls >= 2;
        return new Response(
          JSON.stringify({
            id: "vid-9",
            status: done ? "completed" : "processing",
            model: "m",
            provider: "p",
            progress: done ? 100 : 50,
            created: 1,
            video_url: done ? "https://done.mp4" : null,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        );
      }
      return new Response("nope", { status: 404 });
    });
  }

  it("awaiting resolves to the final completed job", async () => {
    const client = new LiteGenClient({ apiKey: "lg", fetch: jobFetch() });
    const video = await client.videos.generate(
      { prompt: "p", model: "runway/gen-3" },
      { intervalMs: 0 },
    );
    expect(video.status).toBe(GenerationStatus.Completed);
    expect(video.video_url).toBe("https://done.mp4");
  });

  it("for await streams progress including the terminal update", async () => {
    const client = new LiteGenClient({ apiKey: "lg", fetch: jobFetch() });
    const progress: number[] = [];
    for await (const update of client.videos.generate(
      { prompt: "p", model: "runway/gen-3" },
      { intervalMs: 0 },
    )) {
      progress.push(update.progress as number);
    }
    expect(progress).toEqual([50, 100]);
  });

  it("submitted resolves to the initial job id without waiting", async () => {
    const client = new LiteGenClient({ apiKey: "lg", fetch: jobFetch() });
    const submitted = await client.videos.generate({ prompt: "p", model: "runway/gen-3" }).submitted;
    expect(submitted.id).toBe("vid-9");
    expect(submitted.status).toBe(GenerationStatus.Processing);
  });
});

describe("LiteGenClient.models.list", () => {
  it("unwraps the {object,data} envelope", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(JSON.stringify({ object: "list", data: [{ id: "m1" }] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    const client = new LiteGenClient({ fetch: fetchImpl });
    const models = await client.models.list();
    expect(models).toHaveLength(1);
    expect((models[0] as { id: string }).id).toBe("m1");
  });

  it("asks for every served model with { all: true }, alongside a media_type", async () => {
    const urls: string[] = [];
    const fetchImpl = mockFetch(async (req) => {
      urls.push(req.url);
      return new Response(JSON.stringify({ object: "list", data: [] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });
    const client = new LiteGenClient({ fetch: fetchImpl });
    await client.models.list({ all: true });
    await client.models.list({ all: true, media_type: "video" });
    await client.models.list({ media_type: "video" });
    expect(urls[0]).toMatch(/\/v1\/models\?all=true$/);
    expect(urls[1]).toMatch(/\/v1\/models\?media_type=video&all=true$/);
    expect(urls[2]).toMatch(/\/v1\/models\?media_type=video$/);
  });
});

describe("LiteGenClient.generations.get", () => {
  it("GETs /v1/generations/{id} with the id URL-encoded", async () => {
    const captured: Partial<CapturedRequest> = {};
    const fetchImpl = mockFetch(async (req) => {
      Object.assign(captured, req);
      return new Response(
        JSON.stringify({
          id: "litegen-3d/a b",
          key_id: null,
          model: "fal/trellis",
          provider: "fal",
          media_type: "model3d",
          status: "processing",
          progress: 40,
          cost_usd: 0,
          created_at: "2026-09-11T00:00:00Z",
        }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    });
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const gen = await client.generations.get("litegen-3d/a b");
    expect(captured.method).toBe("GET");
    expect(captured.url).toMatch(/\/v1\/generations\/litegen-3d%2Fa%20b$/);
    expect(captured.body).toBeUndefined();
    expect(gen.status).toBe("processing");
    expect(gen.media_type).toBe("model3d");
  });

  it("rejects with LiteGenAPIError carrying the HTTP status (404 = not persisted yet)", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(
          JSON.stringify({ error: { message: "not found", type: "not_found", code: "404" } }),
          { status: 404, headers: { "Content-Type": "application/json" } },
        ),
    );
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    await expect(client.generations.get("missing")).rejects.toMatchObject({
      name: "LiteGenAPIError",
      status: 404,
    });
  });
});

describe("enum constants", () => {
  it("matches API string values exactly", () => {
    expect(GenerationStatus.Completed).toBe("completed");
    expect(GenerationStatus.Failed).toBe("failed");
    expect(RefImageKind.Url).toBe("url");
    expect(RefImageKind.Base64).toBe("base64");
    expect(RefImageKind.Generation).toBe("generation");
  });
});

describe("LiteGenClient.keys", () => {
  it("create posts {name} and returns the created key", async () => {
    const fetchImpl = mockFetch(async (req) => {
      expect(req.url).toContain("/v1/keys");
      expect(req.method).toBe("POST");
      expect((req.body as { name: string }).name).toBe("my-key");
      return new Response(
        JSON.stringify({
          key: "lg-abc",
          prefix: "lg-abc12",
          name: "my-key",
          created_at: "2026-05-28T00:00:00Z",
        }),
        { status: 201, headers: { "Content-Type": "application/json" } },
      );
    });
    const client = new LiteGenClient({ fetch: fetchImpl });
    const resp = await client.keys.create("my-key");
    expect(resp.key).toBe("lg-abc");
  });

  it("list unwraps the {data} envelope", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(
          JSON.stringify({
            data: [
              {
                id: "00000000-0000-0000-0000-000000000001",
                name: "k1",
                prefix: "lg-aaa",
                created_at: "2026-05-28T00:00:00Z",
                is_active: true,
              },
            ],
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
    );
    const client = new LiteGenClient({ fetch: fetchImpl });
    const keys = await client.keys.list();
    expect(keys).toHaveLength(1);
    expect(keys[0]?.name).toBe("k1");
  });
});

describe("Idempotency-Key", () => {
  // One key per paid request: a retry after a crash with the same key gets
  // the first response back and is billed nothing.
  function capture(responseBody: unknown) {
    const seen: CapturedRequest[] = [];
    const fetchImpl = mockFetch(async (req) => {
      seen.push(req);
      return new Response(JSON.stringify(responseBody), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    });
    return { seen, client: new LiteGenClient({ apiKey: "lg", fetch: fetchImpl }) };
  }

  it("is sent on images.generate", async () => {
    const { seen, client } = capture({ created: 1, data: [], model: "m", provider: "p", id: "litegen-img-1" });
    await client.images.generate({ model: "mock/image-gen", prompt: "x" }, { idempotencyKey: "ledger-1" });
    expect(seen[0]!.headers["idempotency-key"]).toBe("ledger-1");
  });

  it("keepOutputs: false sends X-Litegen-Keep-Outputs on every generation submit", async () => {
    const { seen, client } = capture({ created: 1, data: [], model: "m", provider: "p", id: "litegen-img-1" });
    await client.images.generate({ model: "mock/image-gen", prompt: "x" }, { keepOutputs: false });
    expect(seen[0]!.headers["x-litegen-keep-outputs"]).toBe("false");
    await client.images.generate({ model: "mock/image-gen", prompt: "x" });
    expect(seen[1]!.headers["x-litegen-keep-outputs"]).toBeUndefined();

    const job = { id: "litegen-3d-1", status: "completed", model: "m", provider: "p", progress: 100, created: 1, assets: [{ kind: "mesh", url: "https://x/model.glb", format: "glb" }] };
    const three = capture(job);
    await three.client.models3d.generate({ model: "mock/mesh-3d", prompt: "x" }, { keepOutputs: false, intervalMs: 1 });
    expect(three.seen[0]!.headers["x-litegen-keep-outputs"]).toBe("false");

    const vid = capture({ id: "litegen-vid-1", status: "completed", model: "m", provider: "p", progress: 100, created: 1, video_url: "https://x.mp4" });
    await vid.client.videos.generate({ model: "mock/video-gen", prompt: "x" }, { keepOutputs: false, intervalMs: 1 });
    expect(vid.seen[0]!.headers["x-litegen-keep-outputs"]).toBe("false");
  });

  it("deletes a generation and revokes links by their routes", async () => {
    const { seen, client } = capture({ revoked_at: "2026-11-03T12:00:00.000Z" });
    await client.generations.revokeLinks("litegen-img-1");
    await client.apps.access.revokeLinks("app-1");
    await client.admin.revokeLinks();
    await client.generations.delete("litegen-img-1");
    expect(seen.map((r) => `${r.method} ${new URL(r.url).pathname}`)).toEqual([
      "POST /v1/generations/litegen-img-1/revoke-links",
      "POST /v1/apps/app-1/access/revoke-links",
      "POST /v1/admin/revoke-links",
      "DELETE /v1/generations/litegen-img-1",
    ]);
  });

  it("is sent on models3d.generate and videos.generate submits only", async () => {
    const job = { id: "litegen-3d-1", status: "completed", model: "m", provider: "p", progress: 100, created: 1, assets: [{ kind: "mesh", url: "https://x/model.glb", format: "glb" }] };
    const { seen, client } = capture(job);
    await client.models3d.generate({ model: "mock/mesh-3d", prompt: "x" }, { idempotencyKey: "ledger-3d", intervalMs: 1 });
    expect(seen[0]!.headers["idempotency-key"]).toBe("ledger-3d");
    for (const r of seen.slice(1)) expect(r.headers["idempotency-key"]).toBeUndefined();

    const vid = capture({ id: "litegen-vid-1", status: "completed", model: "m", provider: "p", progress: 100, created: 1, video_url: "https://x.mp4" });
    await vid.client.videos.generate({ model: "mock/video-gen", prompt: "x" }, { idempotencyKey: "ledger-vid", intervalMs: 1 });
    expect(vid.seen[0]!.headers["idempotency-key"]).toBe("ledger-vid");
  });

  it("is not sent when not given", async () => {
    const { seen, client } = capture({ created: 1, data: [], model: "m", provider: "p", id: "litegen-img-1" });
    await client.images.generate({ model: "mock/image-gen", prompt: "x" });
    expect(seen[0]!.headers["idempotency-key"]).toBeUndefined();
  });
});
