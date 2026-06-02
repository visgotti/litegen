import { describe, it, expect, vi } from "vitest";
import {
  LiteGenClient,
  LiteGenAPIError,
  GenerationStatus,
  RefImageKind,
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
});

describe("enum constants", () => {
  it("matches API string values exactly", () => {
    expect(GenerationStatus.Completed).toBe("completed");
    expect(GenerationStatus.Failed).toBe("failed");
    expect(RefImageKind.Url).toBe("url");
    expect(RefImageKind.Base64).toBe("base64");
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
