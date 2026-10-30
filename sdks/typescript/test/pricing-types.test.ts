import { describe, expect, expectTypeOf, it } from "vitest";
import type { ModelInfo, PriceRule } from "../src";

// Compile-time checks on prices that follow the request (PixelGenie 3D doc
// §3G, L14), from the OpenAPI document — `tsc --noEmit` covers this file.
describe("pricing rule types", () => {
  it("types a model's rules on its listing", () => {
    expectTypeOf<NonNullable<ModelInfo["pricing"]>["variable_pricing"]>().toEqualTypeOf<PriceRule[] | undefined>();
    const perSecond: PriceRule = { when: { resolution: "1080p" }, per_second_usd: 0.2 };
    expect(perSecond.per_second_usd).toBe(0.2);
  });
});
