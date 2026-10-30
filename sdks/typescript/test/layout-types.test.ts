import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, LayoutItem, LayoutKind, ModelCapabilities } from "../src";

// Per-box prompts and masks named in text (PixelGenie 3D doc §4) —
// `tsc --noEmit` covers this file.
describe("layout and mask_prompt types", () => {
  it("types the boxes, the text mask and the catalog flags", () => {
    expectTypeOf<ImageGenerationRequest["layout"]>().toEqualTypeOf<LayoutItem[] | null | undefined>();
    expectTypeOf<ImageGenerationRequest["mask_prompt"]>().toEqualTypeOf<string | null | undefined>();
    expectTypeOf<ModelCapabilities["layout_kinds"]>().toEqualTypeOf<LayoutKind[] | undefined>();
    expectTypeOf<ModelCapabilities["supports_mask_prompt"]>().toEqualTypeOf<boolean | undefined>();
    const request: ImageGenerationRequest = {
      model: "recraft/recraftv3",
      prompt: "a shop sign",
      layout: [{ box: { x: 0.3, y: 0.45, w: 0.3, h: 0.1 }, kind: "text", text: "OPEN" }],
    };
    expect(request.layout?.[0]?.kind).toBe("text");
  });
});
