import { describe, it, expect } from "vitest";
import { LiteGenClient } from "../src";

interface CapturedRequest {
  url: string;
  method: string;
  headers: Record<string, string>;
}

const CSV_BODY =
  "id,model,provider,status,cost_usd\nlitegen-1,runway/gen-3,runway,completed,0.12\n";

function csvMockFetch(
  capture: (req: CapturedRequest) => void,
): typeof fetch {
  return ((async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers: Record<string, string> = {};
    const h = new Headers(init?.headers);
    h.forEach((v, k) => {
      headers[k.toLowerCase()] = v;
    });
    capture({ url, method: init?.method ?? "GET", headers });
    return new Response(CSV_BODY, {
      status: 200,
      headers: { "Content-Type": "text/csv" },
    });
  }) as unknown) as typeof fetch;
}

describe("logs.exportCsv", () => {
  it("requests format=csv with filters + active-tenant + auth headers and returns a Blob", async () => {
    let captured: CapturedRequest | undefined;
    const fetchImpl = csvMockFetch((r) => {
      captured = r;
    });
    const client = new LiteGenClient({ apiKey: "lg-secret", fetch: fetchImpl });
    client.setActiveTenant("org-X", "app-Y");

    const blob = await client.logs.exportCsv({
      model: "runway/gen-3",
      provider: "runway",
      status: "completed",
      from: "2026-01-01",
      to: "2026-02-01",
    });

    // (a) URL contains format=csv and the filter params.
    expect(captured!.url).toContain("/v1/logs");
    expect(captured!.url).toContain("format=csv");
    expect(captured!.url).toContain("model=runway%2Fgen-3");
    expect(captured!.url).toContain("provider=runway");
    expect(captured!.url).toContain("status=completed");
    expect(captured!.url).toContain("from=2026-01-01");
    expect(captured!.url).toContain("to=2026-02-01");

    // (b) headers include active tenant + Authorization.
    expect(captured!.headers["x-litegen-org-id"]).toBe("org-X");
    expect(captured!.headers["x-litegen-app-id"]).toBe("app-Y");
    expect(captured!.headers["authorization"]).toBe("Bearer lg-secret");

    // (c) returns the CSV body as a Blob.
    expect(blob).toBeInstanceOf(Blob);
    expect(await blob.text()).toBe(CSV_BODY);
  });
});

describe("audit.exportCsv", () => {
  it("requests format=csv with filters + active-tenant + auth headers and returns a Blob", async () => {
    let captured: CapturedRequest | undefined;
    const fetchImpl = csvMockFetch((r) => {
      captured = r;
    });
    const client = new LiteGenClient({ apiKey: "lg-secret", fetch: fetchImpl });
    client.setActiveTenant("org-X", "app-Y");

    const blob = await client.audit.exportCsv({
      actor_key_id: "key-1",
      action: "key.create",
      from: "2026-01-01",
      to: "2026-02-01",
    });

    expect(captured!.url).toContain("/v1/audit");
    expect(captured!.url).toContain("format=csv");
    expect(captured!.url).toContain("actor_key_id=key-1");
    expect(captured!.url).toContain("action=key.create");
    expect(captured!.url).toContain("from=2026-01-01");
    expect(captured!.url).toContain("to=2026-02-01");

    expect(captured!.headers["x-litegen-org-id"]).toBe("org-X");
    expect(captured!.headers["x-litegen-app-id"]).toBe("app-Y");
    expect(captured!.headers["authorization"]).toBe("Bearer lg-secret");

    expect(blob).toBeInstanceOf(Blob);
    expect(await blob.text()).toBe(CSV_BODY);
  });
});
