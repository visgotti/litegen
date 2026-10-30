import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, ModelCapabilities } from "../src";

// Opt-in pixel preservation (PixelGenie 3D doc §4) — `tsc --noEmit` covers
// this file.
describe("preserve_unmasked types", () => {
  it("types the two fields and the catalog flag", () => {
    expectTypeOf<ImageGenerationRequest["preserve_unmasked"]>().toEqualTypeOf<boolean | null | undefined>();
    expectTypeOf<ImageGenerationRequest["feather_px"]>().toEqualTypeOf<number | null | undefined>();
    expectTypeOf<ModelCapabilities["supports_preserve_unmasked"]>().toEqualTypeOf<boolean | undefined>();
    const request: ImageGenerationRequest = {
      model: "stability/inpaint",
      prompt: "a lamp",
      region: { x: 10, y: 8, w: 20, h: 12 },
      preserve_unmasked: true,
      feather_px: 3,
    };
    expect(request.feather_px).toBe(3);
  });
});
