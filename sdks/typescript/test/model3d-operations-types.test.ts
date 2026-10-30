import { describe, expect, expectTypeOf, it } from "vitest";
import type { Model3dGenerationRequest, ModelCapabilities, SceneManifest, ScenePart } from "../src";

// Texture, remesh and segment (PixelGenie 3D doc §3F) — `tsc --noEmit`
// covers this file.
describe("3D operation types", () => {
  it("types a texture request and the sizes a model paints at", () => {
    expectTypeOf<Model3dGenerationRequest["texture_resolution"]>().toEqualTypeOf<number | null | undefined>();
    expectTypeOf<Model3dGenerationRequest["keep_uvs"]>().toEqualTypeOf<boolean | null | undefined>();
    expectTypeOf<ModelCapabilities["texture_resolutions"]>().toEqualTypeOf<number[] | undefined>();
    expectTypeOf<ModelCapabilities["supports_keep_uvs"]>().toEqualTypeOf<boolean | undefined>();
    const request: Model3dGenerationRequest = {
      model: "mock/texture-3d",
      prompt: "rusty iron",
      source_model: { type: "generation", value: "litegen-3d-1" },
      pbr: true,
      texture_resolution: 1024,
    };
    expect(request.texture_resolution).toBe(1024);
  });

  it("types a segmented mesh's parts", () => {
    expectTypeOf<SceneManifest["parts"]>().toEqualTypeOf<ScenePart[] | null | undefined>();
    const part: ScenePart = { name: "part_1", node: 1, triangle_count: 6 };
    expect(part.name).toBe("part_1");
  });
});
