import { describe, expect, expectTypeOf, it } from "vitest";
import type { components } from "../src/generated/schema";
import type { ModelCapabilities, ModelInfo, ModelSchema, ModelStatus, PromptSpec } from "../src";

type ModelListResponse = components["schemas"]["ModelListResponse"];

// Compile-time checks on the truthful catalog (L14), from the OpenAPI
// document — `tsc --noEmit` covers this file.
describe("catalog types", () => {
  it("types each model's lifecycle and the listing's version", () => {
    expectTypeOf<ModelInfo["status"]>().toEqualTypeOf<ModelStatus>();
    expectTypeOf<ModelInfo["retires_at"]>().toEqualTypeOf<string | null | undefined>();
    expectTypeOf<ModelInfo["replaced_by"]>().toEqualTypeOf<string | null | undefined>();
    expectTypeOf<ModelListResponse["catalog_version"]>().toEqualTypeOf<string>();
    expectTypeOf<ModelCapabilities["max_reference_images"]>().toEqualTypeOf<number | undefined>();
    const status: ModelStatus = "deprecated";
    expect(status).toBe("deprecated");
  });

  it("types what a served schema adds: litegen's own params, typed extras, its status and the body cap", () => {
    expectTypeOf<ModelSchema["litegen_params"]>().toEqualTypeOf<{ [key: string]: unknown } | undefined>();
    expectTypeOf<ModelSchema["extra_params"]>().toEqualTypeOf<{ [key: string]: unknown } | undefined>();
    expectTypeOf<ModelSchema["status"]>().toEqualTypeOf<ModelStatus | undefined>();
    expectTypeOf<PromptSpec["required_for_text_to_video"]>().toEqualTypeOf<boolean | undefined>();
    // On every listed model, so required in the schema: a number, never undefined.
    expectTypeOf<ModelCapabilities["max_request_bytes"]>().toEqualTypeOf<number>();
    // The server computes `litegen_params` (readOnly in the OpenAPI document).
    const served: ModelSchema = { litegen_params: {} } as ModelSchema;
    // @ts-expect-error — read-only
    served.litegen_params = { ground: { kind: "bool" } };
    expect(served.litegen_params).toBeDefined();
  });
});
