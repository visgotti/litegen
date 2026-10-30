import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, ModelCapabilities, ModelCapabilityFlags } from "../src";

// Background removal (PixelGenie L8) is an edit: one `init` image on
// images.generate. The catalog says which models do it — `tsc --noEmit`
// covers this file.
describe("background removal types", () => {
  it("types the catalog flag and the listed capability", () => {
    expectTypeOf<ModelCapabilityFlags["background_removal"]>().toEqualTypeOf<boolean | undefined>();
    expectTypeOf<ModelCapabilities["supports_background_removal"]>().toEqualTypeOf<boolean | undefined>();
  });

  it("is requested like any edit, with only an init image", () => {
    const request: ImageGenerationRequest = {
      model: "fal/bria-rmbg-2",
      reference_images: [{ type: "url", value: "https://example.com/sprite.png", role: "init" }],
    };
    expect(request.reference_images?.[0]?.role).toBe("init");
  });
});
