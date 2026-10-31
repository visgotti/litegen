import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, ImageResult, ModelCapabilities, PixelSize } from "../src";

// Native pixel art (PixelGenie L16) — `tsc --noEmit` covers this file.
describe("pixel art types", () => {
  it("types the palette, directions and catalog fields", () => {
    expectTypeOf<ImageGenerationRequest["palette"]>().toEqualTypeOf<string[] | null | undefined>();
    expectTypeOf<ImageGenerationRequest["directions"]>().toEqualTypeOf<number | null | undefined>();
    expectTypeOf<ImageResult["direction"]>().toEqualTypeOf<string | null | undefined>();
    expectTypeOf<ModelCapabilities["pixel_size"]>().toEqualTypeOf<PixelSize | null | undefined>();
    const request: ImageGenerationRequest = { model: "pixellab/pixflux", prompt: "a knight", size: "64x64", palette: ["#081820", "#346856"] };
    expect(request.palette).toHaveLength(2);
  });
});
