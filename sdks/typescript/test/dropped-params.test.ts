import { describe, expect, it, vi } from "vitest";
import { DROPPED_PARAMS_HEADER, LiteGenAPIError, LiteGenClient, parseDroppedParams } from "../src";

function clientWith(fetchImpl: typeof fetch) {
  return new LiteGenClient({ baseUrl: "https://litegen.test", apiKey: "sk_test", fetch: fetchImpl });
}

/** A JSON 200, optionally carrying the dropped-params header the generation POSTs set. */
function jsonResponse(body: unknown, dropped?: string) {
  const headers: Record<string, string> = { "content-type": "application/json" };
  if (dropped !== undefined) headers[DROPPED_PARAMS_HEADER] = dropped;
  return new Response(JSON.stringify(body), { status: 200, headers });
}

describe("parseDroppedParams", () => {
  it("splits the comma-joined header the server writes", () => {
    expect(parseDroppedParams("rig,symmetry,reference_images[view-top]")).toEqual([
      "rig",
      "symmetry",
      "reference_images[view-top]",
    ]);
  });

  it("reads a missing or empty header as nothing dropped", () => {
    expect(parseDroppedParams(null)).toEqual([]);
    expect(parseDroppedParams(undefined)).toEqual([]);
    expect(parseDroppedParams("")).toEqual([]);
  });

  it("tolerates stray whitespace and empty entries", () => {
    expect(parseDroppedParams(" rig , ,pbr ")).toEqual(["rig", "pbr"]);
  });
});

describe("droppedParams on a job handle", () => {
  it("surfaces what lax mode dropped on a 3D submit", async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse({ id: "litegen-3d-1", status: "pending", progress: 0 }, "rig,symmetry"),
    );
    const job = clientWith(fetchMock as unknown as typeof fetch).models3d.generate({
      model: "mock/mesh-3d",
      prompt: "a fox",
      strict: false,
      rig: true,
      symmetry: "auto",
    } as never);

    expect(await job.droppedParams).toEqual(["rig", "symmetry"]);
    expect((await job.submitted).id).toBe("litegen-3d-1");
    // `submitted` and `droppedParams` read the same memoized POST — never two.
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("is empty when the response carries no header", async () => {
    const job = clientWith(
      (async () => jsonResponse({ id: "j", status: "pending", progress: 0 })) as unknown as typeof fetch,
    ).models3d.generate({ model: "mock/mesh-3d", prompt: "a fox" } as never);
    expect(await job.droppedParams).toEqual([]);
  });

  it("works the same on a video job", async () => {
    const fetchMock = vi.fn(async (url: string) => {
      expect(String(url)).toContain("/v1/videos/generations");
      return jsonResponse({ id: "litegen-vid-1", status: "pending", progress: 0 }, "fps");
    });
    const job = clientWith(fetchMock as unknown as typeof fetch).videos.generate({
      model: "mock/video",
      prompt: "waves",
      strict: false,
    } as never);
    expect(await job.droppedParams).toEqual(["fps"]);
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it("rejects with the submit error, like `submitted` does", async () => {
    const job = clientWith(
      (async () =>
        new Response(
          JSON.stringify({ error: { message: "rig not supported", type: "validation_error", code: "param_unsupported" } }),
          { status: 400, headers: { "content-type": "application/json" } },
        )) as unknown as typeof fetch,
    ).models3d.generate({ model: "mock/mesh-3d", prompt: "a fox", rig: true } as never);
    await expect(job.droppedParams).rejects.toMatchObject({ status: 400, code: "param_unsupported" });
    await expect(job.submitted).rejects.toMatchObject({ status: 400 });
  });
});

describe("validation errors name the field and model", () => {
  // The body litegen-core's validator sends (err_body): `param` and `model`
  // ride alongside code/message/type.
  const body = {
    error: {
      code: "param_unsupported",
      message: "seed not supported by 'mock/mesh-3d'",
      model: "mock/mesh-3d",
      param: "seed",
      type: "validation_error",
    },
  };

  it("exposes param and model on LiteGenAPIError", async () => {
    const job = clientWith(
      (async () => new Response(JSON.stringify(body), { status: 400, headers: { "content-type": "application/json" } })) as unknown as typeof fetch,
    ).models3d.generate({ model: "mock/mesh-3d", prompt: "a fox", seed: 7 });
    const err = await job.submitted.catch((e: unknown) => e);
    expect(err).toBeInstanceOf(LiteGenAPIError);
    expect(err).toMatchObject({ status: 400, code: "param_unsupported", param: "seed", model: "mock/mesh-3d" });
  });

  it("leaves them undefined when the server names neither", () => {
    const err = new LiteGenAPIError(500, { message: "boom", type: "internal_error" });
    expect(err.param).toBeUndefined();
    expect(err.model).toBeUndefined();
  });
});
