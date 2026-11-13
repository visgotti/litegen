import { describe, expect, expectTypeOf, it } from "vitest";
import { LiteGenClient, type components, type HealthResponse, type MediaType, type ProviderHealth } from "../src";

type ReadinessResponse = components["schemas"]["ReadinessResponse"];

function answering(body: unknown) {
  return new LiteGenClient({
    fetch: (async () =>
      new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } })) as unknown as typeof fetch,
  });
}

describe("health", () => {
  it("names the modality each provider entry checked", () => {
    expectTypeOf<ProviderHealth["modality"]>().toEqualTypeOf<MediaType>();
    expectTypeOf<HealthResponse["providers"]>().toEqualTypeOf<ProviderHealth[]>();
  });

  it("types readiness as the server answers it: the database, and the providers set up with the gateway's keys", async () => {
    const body: ReadinessResponse = { status: "ready", checks: { db: true, providers: ["fal", "openai"] } };
    const ready = await answering(body).health.ready();
    expectTypeOf(ready).toEqualTypeOf<ReadinessResponse>();
    expect(ready.checks.providers).toEqual(["fal", "openai"]);
  });
});
