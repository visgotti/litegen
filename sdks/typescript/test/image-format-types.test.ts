import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageBackground, ImageGenerationRequest, ImageOutputFormat, ModelCapabilities } from "../src";

// Compile-time checks on output format and transparency (PixelGenie L9), from
// the OpenAPI document — `tsc --noEmit` covers this file.
describe("image format types", () => {
  it("types the container and background a caller asks for", () => {
    expectTypeOf<ImageGenerationRequest["output_format"]>().toEqualTypeOf<ImageOutputFormat | null | undefined>();
    expectTypeOf<ImageGenerationRequest["background"]>().toEqualTypeOf<ImageBackground | null | undefined>();
    const request: ImageGenerationRequest = { model: "openai/gpt-image-2", prompt: "a sprite", output_format: "png", background: "transparent" };
    expect(request.background).toBe("transparent");
  });

  it("types what a model can return", () => {
    expectTypeOf<ModelCapabilities["supports_transparent_background"]>().toEqualTypeOf<boolean | undefined>();
    expectTypeOf<ModelCapabilities["lossy_only"]>().toEqualTypeOf<boolean | undefined>();
  });
});
