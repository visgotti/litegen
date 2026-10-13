# Touch points

This file lists every file a provider change can touch, grouped by type of
change. Use it twice: once to plan, and again as the final audit. Paths are
relative to the repo root. Entries name symbols rather than line numbers,
because line numbers move.

## Contents

- [A. New model on an existing adapter](#a-new-model-on-an-existing-adapter)
- [B. Updating a carried model](#b-updating-a-carried-model)
- [C. Retiring or renaming a model](#c-retiring-or-renaming-a-model)
- [D. New family for an existing vendor](#d-new-family-for-an-existing-vendor)
- [E. New vendor](#e-new-vendor)
- [Consistency sweep](#consistency-sweep)
- [Downstream surfaces](#downstream-surfaces)

## A. New model on an existing adapter

This is the most common change. It usually comes from a `/model-drift` "add"
decision or a direct request.

| # | Where | What |
|---|---|---|
| 1 | Vendor docs | Collect the API page, request schema and pricing page, each with its URL and today's date. |
| 2 | `models/<p>.yaml` | Add the row in the right family section, with `docs_url`, the `@see` comments and the pricing comment. Set params, roles and capabilities exactly as the schema allows. The family skill has a template. |
| 3 | `litegen-core/src/providers/<family>/<p>.rs` | Add a resolution arm, or confirm the verbatim pass-through works. Add endpoint or field gating if this model's schema differs from its siblings'. Put the `@see` triplet (API page, schema, pricing) on the arm. |
| 4 | Same file, `mod tests` | Add a wiremock test for any request shape that differs from the siblings'. A resolution test that iterates the catalog covers the new row automatically; add one if the adapter doesn't have it. |
| 5 | `litegen-core/tests/catalog_conformance.rs` (3D facts go in its "3D family conformance" section, or in `model3d_conformance.rs` if your checkout has one) | Add one test per vendor fact: enums, bounds, prompt limit, required roles, mode-only capabilities, and that `docs_url` is set. Write it first and watch it fail. |
| 6 | `scripts/model-drift/providers/<p>.mjs` | Map the id in `models` (as an array if the adapter routes it to several endpoints). Remove it from `acknowledged` if it was skipped there. Make sure a typings source snapshots its request schema. |
| 7 | `litegen-core/tests/live_providers.rs` | Add a live test if the model brings a request shape that no existing live test covers. |
| 8 | `apps/landing` | Run `node scripts/sync-models.mjs` from inside `apps/landing`. |
| 9 | `docs/superpowers/param-mapping-matrix.md` | Add any new param × provider cells. |
| 10 | `docs/superpowers/<date>-model-drift-decisions.md` | If the change came from a drift decision, tick it with the date. |

## B. Updating a carried model

This covers changes to params, limits or price.

1. Change the conformance fact first, and watch it fail. This is the same
   recipe as the model-drift skill's typings recipe.
2. Fix the YAML: both the values and the `@see … (date)` evidence. For a
   price change, update the quoted price and the arithmetic too. Changing the
   price of a carried model is the user's decision.
3. If the adapter hard-codes the old value (a default, a snapping table, an
   endpoint gate), fix it as well, with a unit test.
4. If any displayed values changed, run `node scripts/sync-models.mjs` in
   `apps/landing`.

## C. Retiring or renaming a model

**Announced, not yet past:**

- Add `(id, "YYYY-MM-DD", "what happens; successor")` to
  `SCHEDULED_SHUTDOWNS` in `litegen-core/tests/catalog_conformance.rs`, with
  an `@see` pointing at the vendor's deprecations page.
- Prefix the row's `description` with `DEPRECATED:` and the date. Conformance
  enforces this for OpenAI rows; do it for every vendor.
- Recommend a successor in your report. The user decides.

**On or after the date:** from the day after the date,
`models_past_their_announced_shutdown_are_removed` fails. When that happens:

- Delete the row from `models/<p>.yaml`.
- Move the id from `SCHEDULED_SHUTDOWNS` into
  `no_retired_model_ids_are_advertised`, with the reason.
- Remove the id from the drift definition's `models`. If the vendor still
  lists it, add an `acknowledged` entry so it doesn't come back as "new".
- Remove its arm from the adapter. A request for a retired id never reaches
  an adapter: the validator answers `model_not_found` first. An arm is worth
  keeping only in an adapter that reads `config.model_mapping`, where it can
  turn a stale operator mapping into a vendor error instead of a different
  billed model.
- Run the [consistency sweep](#consistency-sweep). Retired ids hide in live
  tests, API tests, Playwright specs, landing copy, the README and SDK
  examples.
- If it was the adapter's only live-tested model, repoint the live test.
- If adapter tests still need the retired row (for example to test an
  adapter that keeps serving other rows), move the row into
  `litegen-core/tests/fixtures/retired-models.yaml` and load it with
  `include_str!`, as `image/bedrock.rs` does. Tests that call `ref_schema`
  on a removed id panic.

**Renaming** a model is an add (the new id) plus a retire (the old id), done
in one change.

## D. New family for an existing vendor

An example is adding image generation for a vendor litegen already supports
for video. Do the adapter itself with that family's skill. This table covers
the wiring.

| # | Where | What |
|---|---|---|
| 1 | `litegen-core/src/providers/<family>/<p>.rs`, plus `pub mod <p>;` in that directory's `mod.rs` | Write the new adapter with the same auth scheme and credential fields (a signing `service` may differ by product), the same `api_base` default (when the vendor serves both families from one host) and the same credential handling. Logic both adapters need, such as id translation, uploads, error-envelope parsing and host or region selection, goes into `pub(crate)` functions in one adapter that the other imports. `video/bytedance.rs` imports `image::bytedance::ark_model_id` and `video/leonardo.rs` imports `image::leonardo::upload_init_image` this way. Auth helpers go in `providers/auth/`. |
| 2 | `litegen-core/src/proxy/registry.rs` | Add the import, a `build_<family>_provider` match arm, and the name in `IMAGE_PROVIDERS`, `VIDEO_PROVIDERS` or `MODEL3D_PROVIDERS`. `provider_catalog()` derives `modalities` from those lists, so the dashboard shows the new family with no other change. `provider_catalog_covers_registry` and `model3d_providers_const_matches_the_build_arms` prove that every *listed* name builds. They don't catch a build arm you forgot to add to its list, which would leave the family missing from the dashboard form, so check the list by hand. Some registry tests use real vendors as fixed fixtures: stability as image-only and without 3D, pixverse as video-only, bedrock as image+video. They are `provider_catalog_covers_registry`, `register_provider_populates_both_maps_for_dual_vendor`, `catalog_reports_the_model3d_modality` and `registers_and_resolves_a_model3d_provider`. If your change gives one of those vendors a new family, update those assertions and move the single-family example to a vendor that still has only one family. |
| 3 | Config | Nothing to do: the vendor's env vars (`load_provider_env_overrides` in `config/mod.rs`) and its `providers.<p>` block already apply to every family. |
| 4 | `models/<p>.yaml` | Add the new family's rows as a new section of the same file. Ids must not collide with the other family's; Kling uses `kling/video-…` for video because the vendor reuses model names across families. |
| 5 | Tests | Everything in section A, plus a BYO test showing the vendor's stored credential configures the new adapter: `<family>_provider_for_request("<p>", Some(ProviderCredentials::from_json(…)))`. |
| 6 | `scripts/model-drift/providers/<p>.mjs` | In the same definition, add sources for the new family's endpoints and map its models. |
| 7 | Landing | Update `FALLBACK_CAPABILITIES` (the `outputs` flags) in `apps/landing/scripts/derive-capabilities.mjs`, and the `image`/`video` flags of `PROVIDERS` in `apps/landing/src/config/site.ts` (there is no 3D flag). Then run `node scripts/sync-models.mjs` and `LITEGEN_API_URL=<running gateway> node scripts/sync-capabilities.mjs`. The committed `capabilities.generated.ts` only changes when you run that second command against a live gateway, which is how fal's `model3d: false` went stale. `derive-capabilities.test.mjs` ("baked fallback covers all 18 diagram providers") has its own hard-coded provider list and requires `outputs.image || outputs.video` for each one, so a 3D-only vendor fails it. Extend the list and the assertion when you add a vendor. |
| 8 | `README.md` | Update the "Supported Providers" table and the roadmap. |
| 9 | Live test and matrix | Add one live test for the new family, and its param-matrix cells. |
| 10 | `/health` | Nothing to change. `ProviderRegistry::health_check_all` covers image, video and 3D providers, so the new family shows up once it's registered. |

## E. New vendor

A new vendor needs everything in section D, plus:

| # | Where | What |
|---|---|---|
| 1 | `litegen-core/src/providers/auth/` | Only if the vendor needs a new scheme. Add an `AuthSpec` variant with `scheme_name`, `required_fields` and `is_satisfied_by`, apply it (in `apply` or a signer module), and unit-test it against the vendor's own signing example. |
| 2 | `provider_catalog()` in `proxy/registry.rs` | Only for credentials other than a single API key. Add an arm with `auth_scheme`, `pool_field: "credential_sets"` and the fields to collect; the bedrock, hunyuan and kling arms are examples. Vendors that use a header or bearer key get the generic API-key form automatically. |
| 3 | `load_provider_env_overrides` in `litegen-core/src/config/mod.rs` | Map env vars to `providers.<p>.api_key`, `.api_keys`, `.key_id`, `.key_secret`, `.region` and `.api_base`. Use the vendor's own env var name where it has one. |
| 4 | `litegen.example.yaml` | Add a `providers.<p>` block, commented with the env var, the auth header and any region or host notes. |
| 5 | `litegen-core/.env.example` | Add the env vars the live tests read. |
| 6 | `scripts/model-drift/providers/<p>.mjs` | Write a new definition (see drift-and-pricing.md) and run its two-pass check. |
| 7 | Landing (paths under `apps/landing/`) | Add the vendor to `PROVIDER_DOCS` in `scripts/derive-models.mjs` and to `PROVIDERS` in `src/config/site.ts`. Add a node to `PROVIDER_NODES_RAW` in `src/components/infra-flow/nodes.ts`, with focus-wire rules in `src/components/InfraFlow.module.css`, and a logo in `public/logos/`. Add it to `FALLBACK_CAPABILITIES` in `scripts/derive-capabilities.mjs` and to the provider list in `scripts/derive-capabilities.test.mjs` (see D7). Ask the user before you invent diagram positions or pick a logo. |
| 8 | `apps/price-api` | Leave it unless asked. It is a separate service that seeds only 7 of the 18 providers today, so don't expand it silently. Mention it in your report. |

## Consistency sweep

Run this for every id you added, renamed or removed, and for any new provider
name:

```bash
grep -rn --exclude-dir={node_modules,target,.git,snapshots,dist,out,.next} '<id>' .
```

Never pipe the output through `head`. Expect hits in these places:

- `models/` and the adapter
- `litegen-core/tests/{catalog_conformance,live_providers,multitenant_api}.rs`
- `dashboard/e2e/*.spec.ts`
- `apps/landing/src/config/site.ts`, `apps/landing/src/config/models.generated.ts`,
  `apps/landing/src/components/Hero.tsx` and `apps/landing/scripts/*.test.mjs`
- `README.md` and `litegen.example.yaml`
- `sdks/*/examples` and `sdks/python/README.md`
- `docs/`

Leave generated SDK code (`sdks/typescript/src/generated`,
`sdks/python/litegen/_generated`) and the SDKs' mock-server tests alone. The
ids there are opaque strings that never reach the catalog.

## Downstream surfaces

| Surface | When to update | How |
|---|---|---|
| Landing models table | Any row change | `cd apps/landing && node scripts/sync-models.mjs && node --test scripts/*.test.mjs` (it reads `models/*.yaml` directly; `mock.yaml` is skipped) |
| Landing capability packets | A provider gains or loses a family or an input kind | `LITEGEN_API_URL=http://localhost:4000 node scripts/sync-capabilities.mjs`, run against a live gateway; also edit `FALLBACK_CAPABILITIES` by hand |
| SDKs | Only when the wire contract changes (a new param, schema field or endpoint), never for new rows | `sdks/scripts/regen-all.sh`. Read the notes at the top of the script before running it. |
| Landing API reference | Same as the SDKs | `node scripts/sync-openapi.mjs`, which copies `sdks/openapi.json` into `public/` |
| README | A new vendor or family | Update the "Supported Providers" table and the roadmap. Both are stale today, so fix the rows you touch and mention the rest. |
| price-api | — | Separate service. Mention it in the report; don't change it. |
| Dashboard | Never, for a provider or model | It renders from `GET /v1/providers` and `GET /v1/models`. If you think it needs a change, ask first. |
