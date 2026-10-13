# Testing and verification

A mapping counts as proven only when a test you can name asserts it, and a
mapping counts as working only once a real vendor call has shown it. This
file covers both. Each family skill adds its own required test list on top of
this.

## Contents

- [The layers](#the-layers)
- [Adapter test idioms](#adapter-test-idioms)
- [Rules that keep tests honest](#rules-that-keep-tests-honest)
- [Conformance facts](#conformance-facts)
- [Live verification](#live-verification)
- [The param-mapping matrix](#the-param-mapping-matrix)
- [Commands](#commands)
- [Before you report](#before-you-report)

## The layers

| Layer | Where | Proves |
|---|---|---|
| Adapter unit + wiremock | `#[cfg(test)] mod tests` at the bottom of the adapter | The exact outbound request (method, path, auth, every field name and value, absent fields), how responses and errors are parsed, status mapping, and that resolution and derivation tables return only vendor enum members |
| Catalog conformance | `litegen-core/tests/catalog_conformance.rs`, plus `model3d_conformance.rs` for 3D | Every vendor fact the YAML encodes, as data: enums, bounds, prompt limits, required roles, mode-only capabilities, shutdown dates |
| Registry and auth | `proxy/registry.rs` tests, `providers/auth/mod.rs` tests | The provider builds, sits in the right `*_PROVIDERS` list and has the right credential form; a BYO pool configures it; a new auth scheme matches the vendor's example |
| Drift definition | `scripts/model-drift/test/*.test.mjs` | Every catalog id is mapped, and the definition is in sync with the YAML |
| API (mock provider) | `tests/model3d_api.rs`, `tests/multitenant_api.rs`, `api/middleware/*_tests.rs` | Core behavior. Touch these only when you change core code. |
| Live (billed) | `litegen-core/tests/live_providers.rs` (image, video); `litegen-core/tests/live_model3d.rs` (3D) | The real vendor accepts what we send and returns what we parse |

## Adapter test idioms

`ref_schema` and `make_provider` follow the test modules in `image/bfl.rs`
and `video/vidu.rs`. Those modules have no pool-only helper; the one below
is new, and the closest existing pool test is in `image/openai.rs`.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{Cleanup, MaterializedRef};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The REAL catalog row, so a YAML change breaks the test that relies on it.
    fn ref_schema(id: &str) -> crate::capabilities::ModelSchema {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        crate::capabilities::CapabilityRegistry::from_dir(&p).expect("load").get(id).expect("model").clone()
    }

    /// api_base → wiremock. Configure the way production does.
    fn make_provider(api_base: &str) -> AcmeProvider {
        let mut p = AcmeProvider::new();
        let mut cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() };
        cfg.credentials.api_key = Some("acme-key".to_string());
        p.configure(cfg);
        p
    }

    /// Pool-only, the shape a dashboard/BYO credential arrives in.
    fn make_pooled_provider(api_base: &str) -> AcmeProvider {
        let mut p = AcmeProvider::new();
        let cfg = ProviderInstanceConfig { api_base: Some(api_base.to_string()), ..Default::default() }
            .with_credentials(crate::providers::ProviderCredentials::from_json(
                &serde_json::json!({ "api_keys": [{ "key": "pooled-key" }] }),
            ));
        p.configure(cfg);
        p
    }

    fn empty_refs() -> MaterializedRequest {
        MaterializedRequest { refs: vec![], cleanup: Cleanup::empty() }
    }

    #[tokio::test]
    async fn sends_seed_as_random_seed() {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v2/images"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ /* vendor's documented response */ })))
            .mount(&server).await;

        // …build base/extras with the param set…
        // provider.generate(&ref_schema("acme/photo-2"), &base, &extras, &empty_refs()).await.unwrap();

        let reqs = server.received_requests().await.unwrap();
        let post = reqs.iter().find(|r| r.method == wiremock::http::Method::POST).unwrap();
        assert_eq!(post.headers.get("authorization").unwrap(), "Bearer acme-key");
        let body: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
        assert_eq!(body["random_seed"], 7);        // the vendor's field name AND value
        assert!(body.get("seed").is_none());        // and not our name
    }
}
```

For refs, construct `MaterializedRef { role: "init".into(), form:
MaterializedRefForm::Base64(…) }` directly. Building the form by hand avoids
the network. For a multipart body, assert that the raw body contains the
expected `name="<field>"` parts.

## Rules that keep tests honest

These come from tests in this repo that passed for the wrong reason:

- **Never assert absence vacuously.** Set the input that would produce the
  field, then assert the field is absent. One test checked that
  `aspect_ratio` was missing from the body while its helper never set
  `aspect_ratio` in the first place, so it would have passed against the
  bug.
- **Call the real function; don't copy its table into the test.** An
  integration test once verified the catalog against its own duplicate of the
  adapter's ratio map. A unit test that can call `resolve_ratio` is the one
  that proves the behavior.
- **Take expected values from the vendor's docs, not from your code.** Cite
  the doc in the test's comment. A test written to confirm what the code did
  once asserted the wrong behavior, with a confident comment explaining why it
  was right (fal's `vendor_format`), and only the live run exposed it.
- **Test the real catalog row** with `ref_schema`, not a synthetic
  `ModelSchema`, unless the test is about validator mechanics.
- **Exhaust derivation functions.** A function that maps into a vendor enum
  must be tested over every advertised value, plus unset, unknown, free-form
  and degenerate inputs (`100x0`, `not-a-size`), and asserted to always
  return a member.
- **Name tests as the behavior they pin:** `seed_is_sent_as_random_seed`,
  `cancelled_task_reports_failed_rather_than_pending`,
  `model_field_is_only_sent_to_the_sd3_route`.
- **Write the test first and watch it fail.** A test that never failed has
  not been shown to test anything.

## Conformance facts

There is one test per vendor fact, and it reads the catalog, not the adapter:

```rust
/// Acme Photo 2 `aspect_ratio` enum. "aspect_ratio: one of 1:1, 16:9, 9:16, 4:3, 3:4"
/// @see <https://docs.acme.example/openapi.json> (2026-10-12) — CreateImageRequest
#[test]
fn acme_photo_2_aspect_ratios_are_the_documented_enum() {
    let reg = registry();
    let allowed = aspect_ratios(&reg, "acme/photo-2");
    assert!(!allowed.is_empty(), "acme/photo-2 declares no aspect ratios");
    for ar in &allowed {
        assert!(["1:1", "16:9", "9:16", "4:3", "3:4"].contains(&ar.as_str()),
            "acme/photo-2 allows {ar:?}, not in CreateImageRequest.aspect_ratio");
    }
}
```

- Group the tests under a dated banner,
  `// ─── Acme: vendor facts read 2026-10-12 ───`, as the drift sections do.
- Use the file's helpers: `registry()`, `aspect_ratios()`, `enum_values()`.
- Assert the facts most likely to drift: enums, min/max, prompt limits,
  required roles, which capabilities a model must *not* claim, and the exact
  `extra_allowlist`.
- `models_past_their_announced_shutdown_are_removed` depends on today's date.
  It can be red on any given day for rows you never touched. Read the failure
  before assuming you broke something, and report it rather than "fixing" it
  by editing dates.
- When a row is retired, adapter tests that load it with `ref_schema` panic.
  Move the row into `litegen-core/tests/fixtures/retired-models.yaml` and
  load it with `include_str!`, as `image/bedrock.rs` does. Otherwise repoint
  or delete those tests.

## Live verification

`litegen-core/tests/live_providers.rs` (image and video) and
`litegen-core/tests/live_model3d.rs` (3D: generate, poll, then run the
downloaded GLB through `synthesize_missing_formats`) hit real vendors and cost
real money. Every test is `#[ignore = "live…"]` and returns early when its
credential env var is missing (`skip_if_missing!` in `live_providers.rs`), so a
normal `cargo test` never runs one.
Credentials go in `litegen-core/.env` (git-ignored); the template is
`.env.example`.

- **When to run:** run it if the user asked for live verification or has said
  the spend is fine. Otherwise ask once and state the expected cost: the
  catalog price × the number of generations.
- **What to run:** one generation per family and per distinct request shape
  you added (for example text-to-video and image-to-video when they use
  different endpoints). A cheap model is fine when it exercises the same
  shape.
- **Bar for "verified":** inspect the output; "it returned 200" isn't enough.
  - Image: the bytes decode to the claimed type (PNG `\x89PNG`, JPEG
    `\xFF\xD8`, WEBP `RIFF…WEBP`).
  - Video: the returned URL is fetchable *without vendor credentials* and
    serves `video/*`.
  - 3D: the glb has `glTF` magic, version 2 and a declared length equal to
    its actual length; derived formats agree on triangle and vertex counts.
    The model3d-provider skill has more.
- **Background poller (async families):** prove the poller path once by
  letting a job finish without client polling, as the fal TRELLIS run did.
- **Rejections that must cost nothing:** for each validation rule you added,
  send a request that breaks it and confirm the 4xx comes back before any
  vendor call.
- **Record it:** in the report, give the date, the model, the cost, what was
  checked, and what was *not* exercised. For params, also record it in the
  param-mapping matrix.
- If you didn't run it, write "not verified live", and say which tests stand
  in for it.

## The param-mapping matrix

`docs/superpowers/param-mapping-matrix.md` records, for every param ×
provider cell, what proves the mapping. When you add or change a cell:

- ✅ needs all of the criteria that apply: the YAML advertises it with the
  right kind; a validator test covers the value rule; an adapter test
  asserts the *outbound field name and value*; and a conformance test checks
  enum values against the vendor's set. Name the tests in the cell.
- Otherwise mark it 🟡, implemented but not fully proven, and name the
  missing piece. ⬜ means not implemented.
- Never tick a cell from documentation alone.

## Commands

```bash
M=litegen-core/Cargo.toml

# The adapter you touched (module-path filter)
cargo test --manifest-path $M --lib providers::image::<p>      # or providers::video::<p>, providers::model3d::<p>

# Auth, registry (provider lists, credential form, BYO), catalog loading
cargo test --manifest-path $M --lib providers::auth
cargo test --manifest-path $M --lib proxy::registry
cargo test --manifest-path $M --lib capabilities

# Catalog facts
cargo test --manifest-path $M --test catalog_conformance
cargo test --manifest-path $M --test model3d_conformance          # 3D rows

# Validator and API, only if you touched core
cargo test --manifest-path $M --lib api::middleware
cargo test --manifest-path $M --test model3d_api --test model3d_validation

# Everything offline
cargo test --manifest-path $M

# Lint the files you touched (the branch has pre-existing warnings elsewhere)
cargo clippy --manifest-path $M --all-targets

# Drift definition: tests, then the two-pass check (temp snapshots; leave snapshots/ alone)
node --test 'scripts/model-drift/test/*.test.mjs'
node scripts/model-drift/check.mjs --provider <p> --snapshots "$(mktemp -d)"

# Landing
(cd apps/landing && node scripts/sync-models.mjs && node --test scripts/*.test.mjs)

# Live, billed: only with credentials and a go-ahead
cargo test --manifest-path $M --test live_providers -- --ignored --nocapture live_<p>
cargo test --manifest-path $M --test live_model3d -- --ignored --nocapture        # 3D
```

Other sessions may be editing core files in this shared tree. If a build
fails in a file you didn't touch, check `git status` before you debug it.

## Before you report

- [ ] Adapter tests pass, and each new one failed before the code existed.
- [ ] `catalog_conformance` passes, or the only failure is a date-driven
      shutdown you've reported. For 3D, `model3d_conformance` passes too.
- [ ] `proxy::registry`, `providers::auth` and `capabilities` lib tests pass.
- [ ] The drift tests pass, and the two-pass check shows no `FAILED` and no
      `changed` on its second run.
- [ ] The landing sync has run and the landing tests pass.
- [ ] The consistency sweep is done (see touchpoints.md).
- [ ] The live run is done with its output inspected, or "not verified live"
      is stated.
- [ ] The param-mapping matrix is updated.
