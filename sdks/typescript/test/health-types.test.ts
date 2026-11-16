import { describe, expect, expectTypeOf, it } from "vitest";
import { LiteGenAPIError, LiteGenClient, type components, type HealthResponse, type MediaType, type ProviderHealth } from "../src";

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

/** A client whose fetch answers `status` with `body` as JSON. */
function answeringWith(status: number, body: unknown) {
  return new LiteGenClient({
    fetch: (async () =>
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } })) as unknown as typeof fetch,
  });
}

describe("health.check on a provider that is down", () => {
  // `GET /health` answers 503 with the same body as its 200 when a provider
  // did not check healthy (`status: "degraded"`): a monitoring caller reads it.
  const degraded: HealthResponse = {
    status: "degraded",
    providers: [
      { provider: "openai", modality: "image", healthy: false, latency_ms: 1, last_checked: "2026-11-15T00:00:00Z", message: "connection refused" },
    ],
    cache: { enabled: false, entries: 0 },
  };

  it("answers the 503's health body as the report", async () => {
    await expect(answeringWith(503, degraded).health.check()).resolves.toEqual(degraded);
  });

  it("still throws a 503 without a health body, and any other refusal", async () => {
    const busy = { error: { message: "busy", type: "server_error", code: "server_at_capacity" } };
    const err = await answeringWith(503, busy).health.check().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    expect((err as LiteGenAPIError).status).toBe(503);
    const forbidden = await answeringWith(403, { error: { message: "no", type: "forbidden", code: "forbidden" } }).health.check().catch((e: unknown) => e);
    expect((forbidden as LiteGenAPIError).status).toBe(403);
  });
});
