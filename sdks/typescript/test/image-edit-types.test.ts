import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, MaskConvention, ModelCapabilities, PreservesUnmasked, Region, RegionInput } from "../src";

// Compile-time checks on the partial-edit fields (PixelGenie 3D doc §4), from
// the OpenAPI document — `tsc --noEmit` covers this file.
describe("partial image edit types", () => {
  it("types a region and its growth on the image request", () => {
    expectTypeOf<ImageGenerationRequest["region"]>().toEqualTypeOf<Region | null | undefined>();
    expectTypeOf<ImageGenerationRequest["mask_grow_px"]>().toEqualTypeOf<number | null | undefined>();
    const edit: ImageGenerationRequest = {
      model: "ideogram/ideogram-4-5-precise-edit",
      prompt: "make the visor glow cyan",
      reference_images: [{ type: "base64", value: "iVBORw0KGgo=", role: "init" }],
      region: { x: 384, y: 256, w: 128, h: 64 },
      mask_grow_px: 0,
    };
    expect(edit.region?.w).toBe(128);
  });

  it("types how the listing says a model edits", () => {
    expectTypeOf<ModelCapabilities["region_input"]>().toEqualTypeOf<RegionInput | null | undefined>();
    expectTypeOf<ModelCapabilities["preserves_unmasked"]>().toEqualTypeOf<PreservesUnmasked | null | undefined>();
    expectTypeOf<ModelCapabilities["mask_convention"]>().toEqualTypeOf<MaskConvention | null | undefined>();
    expectTypeOf<ModelCapabilities["supports_region_edit"]>().toEqualTypeOf<boolean | undefined>();
  });
});
