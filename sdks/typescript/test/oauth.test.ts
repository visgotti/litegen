import { describe, it, expect, vi } from "vitest";
import { LiteGenClient } from "../src";

interface Captured {
  url: string;
  method: string;
  body: unknown;
}

function client(answer: unknown, seen: Captured[]): LiteGenClient {
  const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push({
      url: typeof input === "string" ? input : input.toString(),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(init.body as string) : undefined,
    });
    return new Response(JSON.stringify(answer), { status: 200, headers: { "content-type": "application/json" } });
  }) as unknown as typeof fetch;
  return new LiteGenClient({ baseUrl: "https://h/api", fetch: fetchImpl });
}

describe("LiteGenClient.oauth (the MCP consent page's API)", () => {
  it("reads a sign-in request by id", async () => {
    const seen: Captured[] = [];
    const view = { id: "r/1", surface: "ai", requested_scopes: ["ai:read"] };
    const got = await client(view, seen).oauth.getRequest("r/1");
    expect(got).toEqual(view);
    expect(seen[0]).toMatchObject({ method: "GET", url: "https://h/api/v1/oauth/requests/r%2F1" });
  });

  it("approves with the org, app and scopes, and returns where to go", async () => {
    const seen: Captured[] = [];
    const got = await client({ redirect_to: "http://127.0.0.1:5/cb?code=c" }, seen).oauth.approveRequest("r1", {
      org_id: "o1",
      app_id: "a1",
      scopes: ["ai:generate", "ai:read"],
    });
    expect(got.redirect_to).toBe("http://127.0.0.1:5/cb?code=c");
    expect(seen[0]).toEqual({
      method: "POST",
      url: "https://h/api/v1/oauth/requests/r1/approve",
      body: { org_id: "o1", app_id: "a1", scopes: ["ai:generate", "ai:read"] },
    });
  });

  it("denies", async () => {
    const seen: Captured[] = [];
    const got = await client({ redirect_to: "http://127.0.0.1:5/cb?error=access_denied" }, seen).oauth.denyRequest("r1");
    expect(got.redirect_to).toContain("access_denied");
    expect(seen[0]).toMatchObject({ method: "POST", url: "https://h/api/v1/oauth/requests/r1/deny" });
  });
});
