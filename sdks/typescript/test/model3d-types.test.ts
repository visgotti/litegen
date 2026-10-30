import { describe, expect, expectTypeOf, it } from "vitest";
import type {
  Model3dAsset,
  Model3dFormat,
  Model3dGenerationRequest,
  Model3dSymmetry,
  Model3dTopology,
  ParamSpecStringArray,
  ModelInfo,
  ModelSchema,
  ModelTask,
  NativeOrientation,
  SceneManifest,
  SourceRef,
} from "../src";
import { Model3dAssetKind } from "../src";

// Compile-time checks on the 3D request/response types the SDK exports. They
// come from the OpenAPI document (src/generated/schema.d.ts), so a vocabulary
// change in litegen-core shows up here as a type error after regeneration —
// `tsc --noEmit` covers this file (test/ is in tsconfig's include).

describe("3D vocabulary types", () => {
  it("are the closed sets catalog conformance enforces", () => {
    // Every container any 3D model can deliver — a vendor's own, or one litegen
    // converts to locally. A change here is a vocabulary change: update both.
    expectTypeOf<Model3dFormat>().toEqualTypeOf<
      "glb" | "gltf" | "obj" | "stl" | "ply" | "fbx" | "usdz" | "usda" | "3mf" | "dae"
    >();
    expectTypeOf<Model3dTopology>().toEqualTypeOf<"triangle" | "quad">();
    expectTypeOf<Model3dSymmetry>().toEqualTypeOf<"off" | "auto" | "on">();
  });

  it("names every catalog task, on the schema and the listing alike", () => {
    expectTypeOf<ModelTask>().toEqualTypeOf<
      | "generate" | "rig" | "animate" | "texture" | "remesh" | "segment"
      | "multiview" | "background_swap" | "extend" | "retake" | "video_edit"
    >();
    expectTypeOf<ModelSchema["task"]>().toEqualTypeOf<ModelTask | undefined>();
    expectTypeOf<ModelInfo["capabilities"]["task"]>().toEqualTypeOf<ModelTask | undefined>();
  });

  it("types an operation's source_model", () => {
    const chained: Model3dGenerationRequest = { model: "mock/rig-3d", source_model: { type: "generation", value: "litegen-3d-1" } };
    const own: SourceRef = { type: "base64", value: "Z2xURg==", format: "glb" };
    // @ts-expect-error — `blend` is not a container litegen reads.
    const bad: SourceRef = { type: "url", value: "https://x/a.blend", format: "blend" };
    expectTypeOf<SourceRef["type"]>().toEqualTypeOf<"generation" | "url" | "base64">();
    expect([chained, own, bad]).toHaveLength(3);
  });

  it("types output_formats with the format vocabulary", () => {
    const ok: Model3dGenerationRequest = { model: "fal/trellis", output_formats: ["glb", "obj"] };
    // @ts-expect-error — `blend` is not a container any 3D model can deliver.
    const bad: Model3dGenerationRequest = { model: "fal/trellis", output_formats: ["blend"] };
    expect([ok, bad]).toHaveLength(2);
  });

  it("makes the prompt optional — image-only models take none", () => {
    const imageOnly: Model3dGenerationRequest = {
      model: "fal/trellis",
      reference_images: [{ type: "url", value: "https://example.com/chair.png", role: "init" }],
    };
    expect(imageOnly.prompt).toBeUndefined();
  });

  it("keeps an asset's format an open string, so a new texture format still parses", () => {
    expectTypeOf<Model3dAsset["format"]>().toEqualTypeOf<string>();
  });

  it("exposes the asset kinds as constants matching the schema", () => {
    expectTypeOf<Model3dAssetKind>().toEqualTypeOf<Model3dAsset["kind"]>();
    expect(Object.values(Model3dAssetKind)).toEqual(["mesh", "texture", "preview", "original"]);
  });

  it("types a mesh's manifest and a model's proven orientation", () => {
    expectTypeOf<Model3dAsset["scene"]>().toEqualTypeOf<SceneManifest | null | undefined>();
    expectTypeOf<SceneManifest["front"]>().toEqualTypeOf<string>();
    expectTypeOf<NonNullable<SceneManifest["skin"]>["humanoid"]>().toEqualTypeOf<{ [key: string]: string } | null | undefined>();
    expectTypeOf<ModelSchema["native_orientation"]>().toEqualTypeOf<NativeOrientation | null | undefined>();
  });

  it("exposes the server-computed derivable set and the docs link", () => {
    expectTypeOf<ParamSpecStringArray["derivable_values"]>().toEqualTypeOf<string[] | undefined>();
    expectTypeOf<ModelSchema["docs_url"]>().toEqualTypeOf<string | null | undefined>();
  });
});
