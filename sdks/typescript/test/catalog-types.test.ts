import { describe, expect, expectTypeOf, it } from "vitest";
import type { components } from "../src/generated/schema";
import type { ModelCapabilities, ModelInfo, ModelStatus } from "../src";

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
});
