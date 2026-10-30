import { describe, expect, expectTypeOf, it } from "vitest";
import type { ImageGenerationRequest, ImageResult, ImageView, RefInputSpec } from "../src";

// Compile-time checks on the multiview fields (PixelGenie 3D doc §3E), from
// the OpenAPI document — `tsc --noEmit` covers this file.
describe("multiview types", () => {
  it("types the views a multiview image model makes and the view each image shows", () => {
    expectTypeOf<ImageGenerationRequest["views"]>().toEqualTypeOf<ImageView[] | null | undefined>();
    expectTypeOf<ImageResult["view"]>().toEqualTypeOf<ImageView | null | undefined>();
    expectTypeOf<ImageResult["yaw_deg"]>().toEqualTypeOf<number | null | undefined>();
    const request: ImageGenerationRequest = {
      model: "tripo/multiview",
      reference_images: [{ type: "base64", value: "iVBORw0KGgo=", role: "init" }],
      views: ["front", "back-left"],
    };
    expect(request.views).toEqual(["front", "back-left"]);
  });

  it("types a multiview-to-3D model's ordered view list", () => {
    expectTypeOf<RefInputSpec["view_order"]>().toEqualTypeOf<ImageView[] | undefined>();
  });
});
