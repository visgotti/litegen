import { describe, expect, expectTypeOf, it, vi } from "vitest";
import {
  LiteGenClient,
  type ImageSegmentationRequest,
  type ImageSegmentationResponse,
  type ModelCapabilities,
  type SegmentationMask,
  type SegmentationPrompt,
  type SegmentationPromptType,
} from "../src";

// Promptable segmentation (PixelGenie L7): POST /v1/images/segmentations.
describe("images.segment", () => {
  it("types prompts, masks and the catalog fields", () => {
    const prompts: SegmentationPrompt[] = [
      { type: "point", x: 4, y: 5, label: 0 },
      { type: "box", x: 1, y: 2, w: 3, h: 4 },
    ];
    const text: SegmentationPrompt = { type: "text", text: "visor" };
    const request: ImageSegmentationRequest = { model: "mock/segment-image", image: "iVBORw0KGgo=", prompts: [...prompts, text] };
    expect(request.prompts).toHaveLength(3);
    expectTypeOf<SegmentationMask["png_base64"]>().toEqualTypeOf<string>();
    expectTypeOf<SegmentationMask["box"]>().toMatchTypeOf<{ x: number; y: number; w: number; h: number }>();
    expectTypeOf<ImageSegmentationResponse["masks"]>().toEqualTypeOf<SegmentationMask[]>();
    expectTypeOf<ModelCapabilities["segmentation_prompts"]>().toEqualTypeOf<SegmentationPromptType[] | undefined>();
    expectTypeOf<ModelCapabilities["supports_segmentation"]>().toEqualTypeOf<boolean | undefined>();
  });

  it("posts the request, with an Idempotency-Key when given, and returns the masks", async () => {
    const answer: ImageSegmentationResponse = {
      id: "litegen-seg-1",
      created: 1,
      model: "mock/segment-image",
      provider: "mock",
      masks: [{ png_base64: "iVBORw0KGgo=", score: 0.9, box: { x: 1, y: 2, w: 3, h: 4 } }],
    };
    const seen: { url: string; method?: string; headers: Headers; body: unknown }[] = [];
    const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      seen.push({ url: input.toString(), method: init?.method, headers: new Headers(init?.headers), body: JSON.parse(init?.body as string) });
      return new Response(JSON.stringify(answer), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as unknown as typeof fetch;
    const client = new LiteGenClient({ apiKey: "lg", fetch: fetchImpl });
    const request: ImageSegmentationRequest = { model: "mock/segment-image", image: "iVBORw0KGgo=", prompts: [{ type: "box", x: 1, y: 2, w: 3, h: 4 }] };
    const got = await client.images.segment(request, { idempotencyKey: "seg-1" });
    expect(got.masks[0]?.box).toEqual({ x: 1, y: 2, w: 3, h: 4 });
    expect(seen[0]!.url).toMatch(/\/v1\/images\/segmentations$/);
    expect(seen[0]!.method).toBe("POST");
    expect(seen[0]!.body).toEqual(request);
    expect(seen[0]!.headers.get("idempotency-key")).toBe("seg-1");
    await client.images.segment(request);
    expect(seen[1]!.headers.get("idempotency-key")).toBeNull();
  });
});
