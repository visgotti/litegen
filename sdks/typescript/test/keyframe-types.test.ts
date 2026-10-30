import { describe, expect, expectTypeOf, it } from "vitest";
import type { KeyframeTiming, ModelCapabilities, ReferenceImage, VideoGenerationRequest } from "../src";

// Compile-time checks on timed keyframes (PixelGenie 3D doc §5), from the
// OpenAPI document — `tsc --noEmit` covers this file.
describe("timed keyframe types", () => {
  it("types a keyframe's time on its reference", () => {
    expectTypeOf<ReferenceImage["time_s"]>().toEqualTypeOf<number | null | undefined>();
    const request: VideoGenerationRequest = {
      model: "pixverse/v5-multi-transition",
      prompt: "the knight winds up, then strikes",
      reference_images: [
        { type: "url", value: "https://example.com/windup.png", role: "keyframe", time_s: 0 },
        { type: "url", value: "https://example.com/hit.png", role: "keyframe", time_s: 2 },
      ],
    };
    expect(request.reference_images?.[1]?.time_s).toBe(2);
  });

  it("types which models take keyframes, and how many", () => {
    expectTypeOf<ModelCapabilities["supports_keyframes"]>().toEqualTypeOf<boolean | undefined>();
    expectTypeOf<ModelCapabilities["max_keyframes"]>().toEqualTypeOf<number | null | undefined>();
    expectTypeOf<ModelCapabilities["keyframe_timing"]>().toEqualTypeOf<KeyframeTiming | null | undefined>();
  });
});
