import { describe, expect, it, vi } from "vitest";
import { LiteGenAPIError, LiteGenClient } from "../src";

// `models3d.convert` / `models3d.convertFormats` — the standalone mesh
// converter (`/v1/utils/models3d/*`): raw bytes in, raw bytes out, no
// generation and nothing billed.

const GLB = new Uint8Array([0x67, 0x6c, 0x54, 0x46, 2, 0, 0, 0]); // "glTF" magic, version 2

interface Seen {
  url: string;
  method: string;
  contentType: string | null;
  auth: string | null;
  body: unknown;
}

function clientReturning(response: () => Response) {
  const seen: Seen[] = [];
  const fetchImpl = vi.fn(async (url: string, init?: RequestInit) => {
    const headers = new Headers(init?.headers);
    seen.push({
      url: String(url),
      method: init?.method ?? "GET",
      contentType: headers.get("content-type"),
      auth: headers.get("authorization"),
      body: init?.body,
    });
    return response();
  });
  const client = new LiteGenClient({ baseUrl: "https://litegen.test", apiKey: "sk_test", fetch: fetchImpl as unknown as typeof fetch });
  return { client, seen };
}

const objResponse = (headers: Record<string, string> = {}) =>
  new Response("o cube\nv 0.5 -0.5 0.5\n", {
    status: 200,
    headers: { "content-type": "model/obj", ...headers },
  });

describe("models3d.convert", () => {
  it("posts the file as raw bytes, not JSON, with from/to in the query", async () => {
    const { client, seen } = clientReturning(() =>
      objResponse({ "content-disposition": 'attachment; filename="model.obj"' }),
    );
    await client.models3d.convert(GLB, { from: "glb", to: "obj" });
    expect(seen).toHaveLength(1);
    expect(seen[0]!.url).toBe("https://litegen.test/v1/utils/models3d/convert?from=glb&to=obj");
    expect(seen[0]!.method).toBe("POST");
    expect(seen[0]!.contentType).toBe("application/octet-stream");
    expect(seen[0]!.auth).toBe("Bearer sk_test");
    // The exact bytes object goes on the wire — never JSON.stringify'd.
    expect(seen[0]!.body).toBe(GLB);
  });

  it("hands back the file, its format, the server's file name and what it lost", async () => {
    const { client } = clientReturning(() =>
      objResponse({
        "content-disposition": 'attachment; filename="model.obj"',
        "x-litegen-conversion-losses": "materials,uvs",
      }),
    );
    const out = await client.models3d.convert(GLB, { from: "glb", to: "obj" });
    expect(out.format).toBe("obj");
    expect(out.filename).toBe("model.obj");
    expect(out.losses).toEqual(["materials", "uvs"]);
    expect(out.blob.type).toBe("model/obj");
    expect(await out.blob.text()).toContain("o cube");
  });

  it("reports no losses and builds the name itself when those headers are absent", async () => {
    // A cross-origin response can hide Content-Disposition; losses are only
    // sent when something was lost.
    const { client } = clientReturning(() => objResponse());
    const out = await client.models3d.convert(GLB, { from: "glb", to: "stl" });
    expect(out.losses).toEqual([]);
    expect(out.filename).toBe("model.stl");
  });

  it("passes an unknown loss kind through instead of failing", async () => {
    const { client } = clientReturning(() => objResponse({ "x-litegen-conversion-losses": "uvs,some_future_kind" }));
    const out = await client.models3d.convert(GLB, { from: "glb", to: "obj" });
    expect(out.losses).toEqual(["uvs", "some_future_kind"]);
  });

  it("accepts an ArrayBuffer or a Blob as well as a typed array", async () => {
    const { client, seen } = clientReturning(() => objResponse());
    const blob = new Blob([GLB]);
    await client.models3d.convert(GLB.buffer, { from: "glb", to: "obj" });
    await client.models3d.convert(blob, { from: "glb", to: "obj" });
    expect(seen[0]!.body).toBe(GLB.buffer);
    expect(seen[1]!.body).toBe(blob);
  });

  it("rejects with the error the server names, param included", async () => {
    const { client } = clientReturning(() =>
      new Response(
        JSON.stringify({
          error: {
            code: "format_unsupported",
            message: "`to` 'fbx' cannot be converted; supported: glb, obj, stl, ply",
            param: "to",
            type: "validation_error",
          },
        }),
        { status: 400, headers: { "content-type": "application/json" } },
      ),
    );
    const err = await client.models3d.convert(GLB, { from: "glb", to: "fbx" }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    expect(err).toMatchObject({ status: 400, code: "format_unsupported", param: "to" });
  });
});

describe("models3d.convertFormats", () => {
  it("reads the converter's containers and input cap", async () => {
    const { client, seen } = clientReturning(() =>
      new Response(JSON.stringify({ formats: ["glb", "obj", "stl", "ply"], max_input_bytes: 134217728 }), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    );
    const caps = await client.models3d.convertFormats();
    expect(seen[0]!.url).toBe("https://litegen.test/v1/utils/models3d/formats");
    expect(seen[0]!.method).toBe("GET");
    expect(caps.formats).toEqual(["glb", "obj", "stl", "ply"]);
    expect(caps.max_input_bytes).toBe(134217728);
  });
});
