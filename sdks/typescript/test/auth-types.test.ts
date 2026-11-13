import { describe, expect, expectTypeOf, it } from "vitest";
import { LiteGenClient, type components } from "../src";

type MeResponse = components["schemas"]["MeResponse"];

function answering(body: unknown) {
  return new LiteGenClient({
    fetch: (async () =>
      new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } })) as unknown as typeof fetch,
  });
}

describe("auth.me", () => {
  it("types the signed-in person, with the role and permissions they hold in each organization", async () => {
    const body: MeResponse = {
      user: { id: "user-1", email: "owner@example.com", role: "owner" },
      orgs: [{ id: "org-1", name: "Acme", role: "admin", permissions: ["org:read", "org:write", "generation:create"] }],
      active_org: "org-1",
      active_app: null,
    };
    const me = await answering(body).auth.me();
    expectTypeOf(me).toEqualTypeOf<MeResponse>();
    expectTypeOf<MeResponse["orgs"][number]["permissions"]>().toEqualTypeOf<string[]>();
    expectTypeOf(me.active_org).toEqualTypeOf<string | null>();
    expect(me.orgs[0]?.permissions).toContain("org:write");
  });
});
