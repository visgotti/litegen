import { describe, it, expect, expectTypeOf } from "vitest";
import { LiteGenClient, type CancelledGeneration, type components } from "../src";

type Schemas = components["schemas"];

/** A fetch that records each URL and answers `body`. */
function recording(body: unknown): { urls: string[]; fetch: typeof fetch } {
  const urls: string[] = [];
  const fetchImpl = (async (input: RequestInfo | URL) => {
    urls.push(typeof input === "string" ? input : input.toString());
    return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
  }) as unknown as typeof fetch;
  return { urls, fetch: fetchImpl };
}

describe("metadata (L17)", () => {
  it("filters logs and stats by metadata.<key>", async () => {
    const r = recording({ data: [], total: 0, page: 1, per_page: 50, total_pages: 0 });
    const client = new LiteGenClient({ apiKey: "lg-secret", fetch: r.fetch });
    await client.logs.list({ metadata: { project: "castle", asset: "knight" } });
    await client.stats.get({ metadata: { project: "castle" } });
    await client.stats.get();
    await client.stats.get(new AbortController().signal);
    const [logs, stats, bare, withSignal] = r.urls as [string, string, string, string];
    const query = new URL(logs, "http://x").searchParams;
    expect(query.get("metadata.project")).toBe("castle");
    expect(query.get("metadata.asset")).toBe("knight");
    expect(stats).toMatch(/\/v1\/stats\?metadata\.project=castle$/);
    expect(bare).toMatch(/\/v1\/stats\??$/);
    expect(withSignal).toMatch(/\/v1\/stats\??$/);
  });

  it("is a string map on every request and echoed on every answer", () => {
    expectTypeOf<Schemas["VideoGenerationRequest"]["metadata"]>().toEqualTypeOf<{ [key: string]: string } | null | undefined>();
    expectTypeOf<Schemas["ImageSegmentationRequest"]["metadata"]>().toEqualTypeOf<{ [key: string]: string } | null | undefined>();
    for (const echo of [
      {} as Schemas["ImageGenerationResponse"]["metadata"],
      {} as Schemas["VideoGenerationResponse"]["metadata"],
      {} as Schemas["Model3dGenerationResponse"]["metadata"],
      {} as Schemas["ImageSegmentationResponse"]["metadata"],
    ]) {
      expectTypeOf(echo).toEqualTypeOf<{ [key: string]: string } | null | undefined>();
    }
  });
});

describe("cancel (L18)", () => {
  it("resolves to the settled row and whether the vendor took the cancel", async () => {
    const r = recording({ id: "litegen-vid-1", status: "cancelled", cost_usd: 0, vendor_cancelled: true });
    const client = new LiteGenClient({ apiKey: "lg-secret", fetch: r.fetch });
    const cancelled = await client.generations.cancel("litegen-vid-1");
    expectTypeOf(cancelled).toEqualTypeOf<CancelledGeneration>();
    expectTypeOf(cancelled.vendor_cancelled).toEqualTypeOf<boolean>();
    expect(cancelled.vendor_cancelled).toBe(true);
    expectTypeOf<Schemas["CancelledGeneration"]["vendor_cancelled"]>().toEqualTypeOf<boolean>();
  });
});
