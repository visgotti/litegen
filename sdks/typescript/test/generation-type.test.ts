import { describe, it, expect } from "vitest";
import { LiteGenClient, type Generation } from "../src";

interface CapturedRequest {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

function mockFetch(
  handler: (req: CapturedRequest) => Response | Promise<Response>,
): typeof fetch {
  return ((async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers: Record<string, string> = {};
    const h = new Headers(init?.headers);
    h.forEach((v, k) => {
      headers[k.toLowerCase()] = v;
    });
    const body = init?.body ? JSON.parse(init.body as string) : undefined;
    return handler({ url, method: init?.method ?? "GET", headers, body });
  }) as unknown) as typeof fetch;
}

// A backend-shaped Generation row exactly as litegen-core serializes it
// (litegen-core/src/types/mod.rs `pub struct Generation`): note `error_message`
// (NOT `error`), `media_type`, `progress`, `provider_job_id`, `metadata`, and
// `key_id: null`. There is NO `error` and NO `request_body` field.
const backendGeneration = {
  id: "litegen-vid-abc",
  key_id: null,
  model: "runway/gen-3",
  provider: "runway",
  media_type: "video",
  status: "failed",
  progress: 42,
  provider_job_id: "job-xyz",
  result_url: null,
  error_message: "provider rejected the prompt",
  cost_usd: 0.12,
  created_at: "2026-06-01T00:00:00Z",
  completed_at: "2026-06-01T00:01:00Z",
  metadata: { source: "dashboard" },
  org_id: "org-1",
  app_id: "app-1",
};

describe("Generation type matches the backend struct", () => {
  it("surfaces error_message / media_type / progress / nullable key_id from generations.list()", async () => {
    const fetchImpl = mockFetch(
      async () =>
        new Response(
          JSON.stringify({
            data: [backendGeneration],
            total: 1,
            page: 1,
            per_page: 25,
            total_pages: 1,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
    );
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const result = await client.generations.list();
    const gen: Generation = result.data[0]!;

    // These field accesses must typecheck against the real backend shape.
    expect(gen.error_message).toBe("provider rejected the prompt");
    expect(gen.media_type).toBe("video");
    expect(gen.progress).toBe(42);
    expect(gen.provider_job_id).toBe("job-xyz");
    expect(gen.key_id).toBeNull();
    expect(gen.metadata).toEqual({ source: "dashboard" });
    expect(gen.org_id).toBe("org-1");
    expect(gen.app_id).toBe("app-1");
    expect(gen.result_url).toBeNull();

    // Compile-time assertions: these property types must exist on Generation.
    const _errType: string | null | undefined = gen.error_message;
    const _mediaType: string = gen.media_type;
    const _progress: number = gen.progress;
    const _keyId: string | null = gen.key_id;
    void _errType;
    void _mediaType;
    void _progress;
    void _keyId;
  });
});
