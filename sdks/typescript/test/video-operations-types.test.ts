import { describe, expect, expectTypeOf, it } from "vitest";
import type { ExtendOutput, ModelCapabilities, TimeRange, VideoGenerationRequest, VideoGenerationResponse, VideoSegment } from "../src";

// Video operations (PixelGenie 3D doc §5) — `tsc --noEmit` covers this file.
describe("video operation types", () => {
  it("types the source, the window and the segments", () => {
    expectTypeOf<VideoGenerationRequest["time_range"]>().toEqualTypeOf<TimeRange | null | undefined>();
    expectTypeOf<VideoGenerationResponse["segments"]>().toEqualTypeOf<VideoSegment[] | null | undefined>();
    expectTypeOf<ModelCapabilities["extend_output"]>().toEqualTypeOf<ExtendOutput | null | undefined>();
    const request: VideoGenerationRequest = {
      model: "mock/retake-video",
      prompt: "he stumbles",
      source_video: { type: "generation", value: "litegen-vid-1" },
      time_range: { start_s: 0.5, end_s: 1.5 },
      replace: "video",
    };
    expect(request.time_range?.end_s).toBe(1.5);
  });
});
