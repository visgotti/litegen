import { describe, expect, expectTypeOf, it } from "vitest";
import type {
  Model3dAsset,
  Model3dFormat,
  Model3dGenerationRequest,
  Model3dSymmetry,
  Model3dTopology,
  ParamSpecStringArray,
  ModelSchema,
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
    expect(Object.values(Model3dAssetKind)).toEqual(["mesh", "texture", "preview"]);
  });

  it("exposes the server-computed derivable set and the docs link", () => {
    expectTypeOf<ParamSpecStringArray["derivable_values"]>().toEqualTypeOf<string[] | undefined>();
    expectTypeOf<ModelSchema["docs_url"]>().toEqualTypeOf<string | null | undefined>();
  });
});
