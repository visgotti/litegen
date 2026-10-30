import { describe, expect, expectTypeOf, it } from "vitest";
import type { Expand, ImageGenerationRequest, ImageResult, ModelCapabilities } from "../src";

// Expanded canvases (PixelGenie 3D doc §4) — `tsc --noEmit` covers this file.
describe("expand types", () => {
  it("types the sides, the delivered size and the catalog flag", () => {
    expectTypeOf<ImageGenerationRequest["expand"]>().toEqualTypeOf<Expand | null | undefined>();
    expectTypeOf<ImageResult["width"]>().toEqualTypeOf<number | null | undefined>();
    expectTypeOf<ModelCapabilities["supports_expand"]>().toEqualTypeOf<boolean | undefined>();
    const request: ImageGenerationRequest = {
      model: "stability/outpaint",
      reference_images: [{ type: "url", value: "https://example.com/a.png", role: "init" }],
      expand: { left: 64, right: 64 },
    };
    expect(request.expand?.left).toBe(64);
  });
});
