import { describe, expect, expectTypeOf, it } from "vitest";
import type { ModelCapabilities, VideoCamera, VideoGenerationRequest, VideoGenerationResponse, VideoMedia } from "../src";

// Compile-time checks on video controls for sprites (PixelGenie L10), from
// the OpenAPI document — `tsc --noEmit` covers this file.
describe("video control types", () => {
  it("types a still, silent, looping request", () => {
    expectTypeOf<VideoGenerationRequest["camera"]>().toEqualTypeOf<VideoCamera | null | undefined>();
    expectTypeOf<VideoGenerationRequest["audio"]>().toEqualTypeOf<boolean | null | undefined>();
    expectTypeOf<VideoGenerationRequest["loop"]>().toEqualTypeOf<boolean | null | undefined>();
    const request: VideoGenerationRequest = { model: "mock/sprite-video", prompt: "idle", camera: "fixed", audio: false, loop: true };
    expect(request.loop).toBe(true);
  });

  it("types what a finished video reports and what a model takes", () => {
    expectTypeOf<VideoGenerationResponse["media"]>().toEqualTypeOf<VideoMedia | null | undefined>();
    expectTypeOf<ModelCapabilities["supports_loop"]>().toEqualTypeOf<boolean | undefined>();
    expectTypeOf<ModelCapabilities["duration_values"]>().toEqualTypeOf<number[] | undefined>();
  });
});
