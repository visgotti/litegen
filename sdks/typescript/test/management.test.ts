import { describe, it, expect, vi } from "vitest";
import { LiteGenClient } from "../src";

interface Captured {
  url: string;
  method: string;
  body: unknown;
}

/** A client whose fetch records every request and answers with `reply`. */
function recordingClient(reply: unknown = {}) {
  const calls: Captured[] = [];
  const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({
      url: typeof input === "string" ? input : input.toString(),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(init.body as string) : undefined,
    });
    return new Response(JSON.stringify(reply), { status: 200, headers: { "Content-Type": "application/json" } });
  }) as unknown as typeof fetch;
  const client = new LiteGenClient({ baseUrl: "http://gw", fetch: fetchImpl });
  const last = () => {
    const c = calls[calls.length - 1]!;
    const u = new URL(c.url);
    return { method: c.method, path: u.pathname, query: Object.fromEntries(u.searchParams), body: c.body };
  };
  return { client, calls, last };
}

const limits = { max_budget_usd: 10, budget_duration: "30d" as string | null };

describe("limits on keys, orgs, members and apps", () => {
  const cases: Array<[string, (c: LiteGenClient) => Promise<unknown>, string, string, unknown]> = [
    ["keys.limits.get", (c) => c.keys.limits.get("k/1"), "GET", "/v1/keys/k%2F1/limits", undefined],
    ["keys.limits.set", (c) => c.keys.limits.set("k1", limits), "PUT", "/v1/keys/k1/limits", limits],
    ["keys.limits.delete", (c) => c.keys.limits.delete("k1"), "DELETE", "/v1/keys/k1/limits", undefined],
    ["keys.limits.resetSpend", (c) => c.keys.limits.resetSpend("k1"), "POST", "/v1/keys/k1/limits/reset", undefined],
    ["keys.get", (c) => c.keys.get("k1"), "GET", "/v1/keys/k1", undefined],
    ["keys.block", (c) => c.keys.block("k1"), "POST", "/v1/keys/k1/block", undefined],
    ["keys.unblock", (c) => c.keys.unblock("k1"), "POST", "/v1/keys/k1/unblock", undefined],
    ["orgs.limits.get", (c) => c.orgs.limits.get("o1"), "GET", "/v1/orgs/o1/limits", undefined],
    ["orgs.limits.set", (c) => c.orgs.limits.set("o1", limits), "PUT", "/v1/orgs/o1/limits", limits],
    ["orgs.limits.delete", (c) => c.orgs.limits.delete("o1"), "DELETE", "/v1/orgs/o1/limits", undefined],
    ["orgs.limits.resetSpend", (c) => c.orgs.limits.resetSpend("o1"), "POST", "/v1/orgs/o1/limits/reset", undefined],
    ["orgs.memberLimits.get", (c) => c.orgs.memberLimits.get("o1", "u1"), "GET", "/v1/orgs/o1/members/u1/limits", undefined],
    ["orgs.memberLimits.set", (c) => c.orgs.memberLimits.set("o1", "u1", limits), "PUT", "/v1/orgs/o1/members/u1/limits", limits],
    ["orgs.memberLimits.delete", (c) => c.orgs.memberLimits.delete("o1", "u1"), "DELETE", "/v1/orgs/o1/members/u1/limits", undefined],
    ["orgs.memberLimits.resetSpend", (c) => c.orgs.memberLimits.resetSpend("o1", "u1"), "POST", "/v1/orgs/o1/members/u1/limits/reset", undefined],
    ["apps.limits.get", (c) => c.apps.limits.get("a1"), "GET", "/v1/apps/a1/limits", undefined],
    ["apps.limits.set", (c) => c.apps.limits.set("a1", limits), "PUT", "/v1/apps/a1/limits", limits],
    ["apps.limits.delete", (c) => c.apps.limits.delete("a1"), "DELETE", "/v1/apps/a1/limits", undefined],
    ["apps.limits.resetSpend", (c) => c.apps.limits.resetSpend("a1"), "POST", "/v1/apps/a1/limits/reset", undefined],
  ];
  for (const [name, call, method, path, body] of cases) {
    it(`${name} → ${method} ${path}`, async () => {
      const { client, last } = recordingClient();
      await call(client);
      expect(last()).toEqual({ method, path, query: {}, body });
    });
  }

  it("limits.list unwraps data and sends filters", async () => {
    const { client, last } = recordingClient({ data: [{ entity_type: "org" }] });
    expect(await client.limits.list()).toEqual([{ entity_type: "org" }]);
    expect(last()).toMatchObject({ method: "GET", path: "/v1/limits", query: {} });
    await client.limits.list({ entityType: "app", orgId: "o2" });
    expect(last().query).toEqual({ entity_type: "app", org_id: "o2" });
  });

  it("limits.me unwraps data", async () => {
    const { client, last } = recordingClient({ data: [{ entity_type: "key" }] });
    expect(await client.limits.me()).toEqual([{ entity_type: "key" }]);
    expect(last().path).toBe("/v1/limits/me");
  });
});

describe("endUsers", () => {
  it("covers CRUD, block and limits, with an optional org", async () => {
    const { client, last } = recordingClient({ data: [{ id: "c1" }] });
    expect(await client.endUsers.list()).toEqual([{ id: "c1" }]);
    expect(last()).toEqual({ method: "GET", path: "/v1/end-users", query: {}, body: undefined });

    await client.endUsers.create({ id: "c1", alias: "Jane" }, { orgId: "o9" });
    expect(last()).toEqual({ method: "POST", path: "/v1/end-users", query: { org_id: "o9" }, body: { id: "c1", alias: "Jane" } });

    const ops: Array<[() => Promise<unknown>, string, string]> = [
      [() => client.endUsers.get("c 1"), "GET", "/v1/end-users/c%201"],
      [() => client.endUsers.update("c1", { blocked: true }), "PATCH", "/v1/end-users/c1"],
      [() => client.endUsers.delete("c1"), "DELETE", "/v1/end-users/c1"],
      [() => client.endUsers.block("c1"), "POST", "/v1/end-users/c1/block"],
      [() => client.endUsers.unblock("c1"), "POST", "/v1/end-users/c1/unblock"],
      [() => client.endUsers.limits.get("c1"), "GET", "/v1/end-users/c1/limits"],
      [() => client.endUsers.limits.set("c1", limits), "PUT", "/v1/end-users/c1/limits"],
      [() => client.endUsers.limits.delete("c1"), "DELETE", "/v1/end-users/c1/limits"],
      [() => client.endUsers.limits.resetSpend("c1", { orgId: "o2" }), "POST", "/v1/end-users/c1/limits/reset"],
    ];
    for (const [call, method, path] of ops) {
      await call();
      expect(last()).toMatchObject({ method, path });
    }
    expect(last().query).toEqual({ org_id: "o2" });
  });
});

describe("spend", () => {
  it("report maps camelCase options to query params", async () => {
    const { client, last } = recordingClient({ data: [] });
    await client.spend.report();
    expect(last()).toMatchObject({ method: "GET", path: "/v1/spend/report", query: {} });
    await client.spend.report({
      groupBy: "end_user",
      from: "2026-10-01",
      to: "2026-10-31",
      orgId: "o1",
      appId: "a1",
      keyId: "k1",
      userId: "u1",
      endUser: "c1",
      model: "openai/gpt-image-1",
      provider: "openai",
      tag: "beta",
    });
    expect(last().query).toEqual({
      group_by: "end_user",
      from: "2026-10-01",
      to: "2026-10-31",
      org_id: "o1",
      app_id: "a1",
      key_id: "k1",
      user_id: "u1",
      end_user: "c1",
      model: "openai/gpt-image-1",
      provider: "openai",
      tag: "beta",
    });
  });

  it("logs pages", async () => {
    const { client, last } = recordingClient({ data: [], total: 0 });
    await client.spend.logs({ page: 2, perPage: 25, tag: "x" });
    expect(last()).toMatchObject({ path: "/v1/spend/logs", query: { page: "2", per_page: "25", tag: "x" } });
  });
});

describe("admin", () => {
  it("models: list/get/create/replace/setDisabled/delete", async () => {
    const { client, last } = recordingClient({ data: [{ id: "m" }] });
    expect(await client.admin.models.list()).toEqual([{ id: "m" }]);
    expect(last().path).toBe("/v1/admin/models");
    const schema = { id: "mock/x", provider: "mock" } as never;
    await client.admin.models.create(schema);
    expect(last()).toMatchObject({ method: "POST", path: "/v1/admin/models", body: schema });
    await client.admin.models.get("mock/x");
    expect(last()).toMatchObject({ method: "GET", path: "/v1/admin/models/mock%2Fx" });
    await client.admin.models.replace("mock/x", schema);
    expect(last()).toMatchObject({ method: "PUT", path: "/v1/admin/models/mock%2Fx", body: schema });
    await client.admin.models.setDisabled("mock/x", true);
    expect(last()).toMatchObject({ method: "PATCH", body: { disabled: true } });
    await client.admin.models.delete("mock/x");
    expect(last()).toMatchObject({ method: "DELETE", path: "/v1/admin/models/mock%2Fx" });
  });

  it("providers: list/get/configure/reset", async () => {
    const { client, last } = recordingClient({ data: [{ name: "openai" }] });
    expect(await client.admin.providers.list()).toEqual([{ name: "openai" }]);
    await client.admin.providers.get("openai");
    expect(last()).toMatchObject({ method: "GET", path: "/v1/admin/providers/openai" });
    await client.admin.providers.configure("openai", { api_key: "sk", api_base: "https://x" });
    expect(last()).toMatchObject({ method: "PUT", path: "/v1/admin/providers/openai", body: { api_key: "sk", api_base: "https://x" } });
    await client.admin.providers.reset("openai");
    expect(last()).toMatchObject({ method: "DELETE", path: "/v1/admin/providers/openai" });
  });

  it("routes and settings", async () => {
    const { client, last } = recordingClient({});
    await client.admin.routes.get();
    expect(last()).toMatchObject({ method: "GET", path: "/v1/admin/routes" });
    const routes = [{ model: "openai/*", deployments: [{ provider: "openai" }] }] as never;
    await client.admin.routes.replace(routes);
    expect(last()).toMatchObject({ method: "PUT", path: "/v1/admin/routes", body: { routes } });
    await client.admin.routes.reset();
    expect(last()).toMatchObject({ method: "DELETE", path: "/v1/admin/routes" });
    await client.admin.settings.get();
    expect(last()).toMatchObject({ method: "GET", path: "/v1/admin/settings" });
    await client.admin.settings.update({ cost_markup_percent: 10 });
    expect(last()).toMatchObject({ method: "PUT", path: "/v1/admin/settings", body: { cost_markup_percent: 10 } });
  });
});
