import { describe, expect, expectTypeOf, it } from "vitest";
import type {
  ConvertModel3dOptions,
  Model3dAsset,
  Model3dDaeMorphs,
  Model3dFormat,
  Model3dGenerationRequest,
  Model3dSymmetry,
  Model3dTopology,
  Model3dUsdClips,
  Model3dUsdzRoot,
  ParamSpecStringArray,
  ModelInfo,
  ModelSchema,
  ModelTask,
  AnimationRequest,
  Model3dAnimationPreset,
  Model3dJointNaming,
  Model3dSkeleton,
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
      "glb" | "gltf" | "obj" | "stl" | "ply" | "fbx" | "usdz" | "usda" | "usdc" | "3mf" | "dae"
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
    // A listed model always carries its task; a schema may be an admin input.
    expectTypeOf<ModelInfo["capabilities"]["task"]>().toEqualTypeOf<ModelTask>();
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
    expect(Object.values(Model3dAssetKind)).toEqual(["mesh", "texture", "preview", "original", "package"]);
  });

  it("types an animate request's clips", () => {
    expectTypeOf<Model3dGenerationRequest["animations"]>().toEqualTypeOf<AnimationRequest[] | null | undefined>();
    expectTypeOf<Model3dGenerationRequest["fps"]>().toEqualTypeOf<number | null | undefined>();
    const walk: AnimationRequest = { name: "walk", preset: "walk", in_place: true, loop: true };
    const preset: Model3dAnimationPreset = "attack_melee";
    expect([walk.name, preset]).toEqual(["walk", "attack_melee"]);
  });

  it("lets a clip leave its name to litegen", () => {
    // Left out or null, litegen names the clip: `walk` after its preset,
    // `clip-<n>` for a prompt clip.
    expectTypeOf<AnimationRequest["name"]>().toEqualTypeOf<string | null | undefined>();
    const clips: AnimationRequest[] = [{ preset: "walk", loop: true }, { prompt: "a slow bow", name: null }];
    expect(clips.map((c) => c.name ?? null)).toEqual([null, null]);
  });

  it("types the rig params from their vocabularies", () => {
    expectTypeOf<Model3dGenerationRequest["skeleton"]>().toEqualTypeOf<Model3dSkeleton | null | undefined>();
    expectTypeOf<Model3dGenerationRequest["joint_naming"]>().toEqualTypeOf<Model3dJointNaming | null | undefined>();
    expectTypeOf<Model3dGenerationRequest["height_m"]>().toEqualTypeOf<number | null | undefined>();
    const rig: Model3dGenerationRequest = {
      model: "tripo/rig",
      source_model: { type: "generation", value: "litegen-3d-1" },
      skeleton: "humanoid",
      joint_naming: "vrm",
    };
    expect(rig.skeleton).toBe("humanoid");
  });

  it("types the Collada morph choice, on a request and on convert", () => {
    expectTypeOf<Model3dDaeMorphs>().toEqualTypeOf<"bake" | "keep">();
    expectTypeOf<Model3dGenerationRequest["dae_morphs"]>().toEqualTypeOf<Model3dDaeMorphs | null | undefined>();
    expectTypeOf<ConvertModel3dOptions["daeMorphs"]>().toEqualTypeOf<Model3dDaeMorphs | undefined>();
    const keep: Model3dGenerationRequest = { model: "meshy/meshy-6", prompt: "a fox", output_formats: ["dae"], dae_morphs: "keep" };
    expect(keep.dae_morphs).toBe("keep");
  });

  it("types the USD choices, on a request and on convert", () => {
    expectTypeOf<Model3dUsdzRoot>().toEqualTypeOf<"usda" | "usdc">();
    expectTypeOf<Model3dUsdClips>().toEqualTypeOf<"all" | "first">();
    expectTypeOf<Model3dGenerationRequest["usdz_root"]>().toEqualTypeOf<Model3dUsdzRoot | null | undefined>();
    expectTypeOf<Model3dGenerationRequest["usd_clips"]>().toEqualTypeOf<Model3dUsdClips | null | undefined>();
    expectTypeOf<ConvertModel3dOptions["usdzRoot"]>().toEqualTypeOf<Model3dUsdzRoot | undefined>();
    expectTypeOf<ConvertModel3dOptions["usdClips"]>().toEqualTypeOf<Model3dUsdClips | undefined>();
    const crate: Model3dGenerationRequest = {
      model: "meshy/meshy-6", prompt: "a fox", output_formats: ["usdz", "usdc"], usdz_root: "usdc", usd_clips: "all",
    };
    expect([crate.usdz_root, crate.usd_clips]).toEqual(["usdc", "all"]);
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
