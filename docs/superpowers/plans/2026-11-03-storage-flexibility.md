# Storage flexibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a deployment, an app or one request keep nothing. Let kept files be
deleted (retention, on request, with the app or org) and their links revoked.
Reach app buckets through an IAM role, and let an app refuse litegen's store as a
fallback.

**Architecture:** Three new layered `storage_access` fields: `keep_outputs`,
`retention_days` and `signed_links`. A keep decision is taken per request and
saved on async rows. litegen's own signed file-route links, plus streaming of
vendor files, make every access mode work without a kept copy. A
`link_revocations` table, `generations.files_deleted_at` and an hourly sweep
handle revocation and retention. An in-crate STS AssumeRole client handles
role buckets.

**Tech Stack:** Rust (axum, sqlx with SQLite and Postgres, rust-s3 0.35,
aws-creds 0.37, hmac/sha2, quick_xml), wiremock; React dashboard
(vitest, Playwright); TS and Python SDKs.

**Spec:** `docs/superpowers/specs/2026-11-03-storage-flexibility-design.md`

## Global Constraints

- No commits: the user commits on their own go. Each "commit" point is a
  ledger line.
- No subagents (the user's rule). The final review is a self-review.
- Don't run cargo concurrently with a peer. Use `CARGO_INCREMENTAL=0`.
  Before each build, check `pgrep -f 'cargo (build|test|check)'`.
- Ping peer `litegen-20` before landing an OpenAPI-visible change.
- Leave `litegen-core/src/mesh/` untouched.
- Never commit `scripts/model-drift/snapshots/`.
- Defaults stay today's behaviour: `keep_outputs: true`,
  `retention_days: 0`, `signed_links: store`,
  `fallback_to_litegen_store: true`, `credential_mode: access_key`.
- Every migration exists for both SQLite and Postgres (`migrations/{sqlite,postgres}/20240101000018_*`, `…19_*`, …).
  Postgres tests run against a throwaway `postgres:16-alpine` container with
  `LITEGEN_PG_TEST_URL`.
- Error codes, verbatim:
  - `invalid_keep_outputs`, `invalid_retention`, `invalid_signed_links`;
  - `invalid_keep_outputs_header`;
  - `format_needs_keeping`;
  - `invalid_link`, `link_expired`, `link_revoked`;
  - `vendor_file_gone`, `files_deleted`;
  - `generation_running`;
  - `invalid_role_arn`, `assume_role_failed`.
- Header, verbatim: `X-Litegen-Keep-Outputs: false`.

## Review Focus

1. **A vendor link that expires before it is read.** The file route answers
   410 `vendor_file_gone`, never a 502 or a hang. Test in Task 5.
2. **A setting changed while a job runs.** Completion honours the decision
   taken at submit. Test in Task 3.
3. **Revocation across replicas.** Two routers sharing one DB both refuse a
   revoked link at once. Test in Task 6.
4. **A retention backlog larger than one batch.** The sweep stops at the batch
   size and the next sweep continues. Test in Task 7.
5. **Role credentials that expire.** A cached credential within 5 minutes of
   expiry is refreshed before use, and a presign longer than the session is
   refused. Test in Task 9.

---

### Task 1: The three layered settings

**Files:**
- Modify: `litegen-core/src/access/mod.rs`, `litegen-core/src/config/mod.rs` (`StorageAccessConfig`), `litegen-core/src/management/types.rs` (`GatewaySettings`, the view), `litegen-core/src/management/settings.rs` (validation), `litegen-core/src/types/mod.rs` (`AppFileAccessRow`, `AppFileAccessUpsert`), `litegen-core/src/db/{sqlite,postgres}.rs`, `litegen-core/src/api/handlers/orgs.rs` (`put_app_access`, `get_app_access`)
- Create: `migrations/{sqlite,postgres}/20240101000018_storage_flexibility.sql`. It adds three nullable columns to `app_file_access`: `keep_outputs`, `retention_days` and `signed_links`.
- Test: unit tests in `access/mod.rs`; `litegen-core/tests/output_access_api.rs`; `db/{sqlite,postgres}_tests.rs`

**Interfaces:**
- Produces:
  - `pub enum SignedLinks { Store, Litegen }`, serialized lowercase.
  - `AccessPolicy { mode, owner_scope, ttl, keep_outputs: bool, retention_days: u32, signed_links: SignedLinks }`.
  - `AccessSettings`, `AppAccessOverrides` and `PutAppAccessRequest` gain the same three fields.
  - `pub const MAX_RETENTION_DAYS: u32 = 36_500`.
  - `GatewaySettings` gains `storage_access_keep_outputs: Option<bool>`, `storage_access_retention_days: Option<u32>` and `storage_access_signed_links: Option<SignedLinks>`.

- [ ] **Step 1: Write the failing tests**
  - `access::tests::the_new_fields_layer_like_the_others`:
    - yaml `keep_outputs: true`, gateway `false`, app none → `false`;
    - app `Some(true)` → `true`;
    - retention follows yaml 0, gateway 30, app 7 → 7;
    - signed_links follows yaml store, app litegen → litegen.
  - `access::tests::with_nothing_set_files_are_public_to_the_key_for_an_hour` is extended: `keep_outputs == true`, `retention_days == 0`, `signed_links == Store`.
  - `access::tests::an_unknown_stored_value_falls_back_to_the_next_layer` is extended: `signed_links: "edge"` and `retention_days: -1` fall through.
  - `config` test `storage_access_new_fields_load_and_bad_ones_refuse_to_start`: `keep_outputs: maybe` is an error, `retention_days: 40000` is an error, `signed_links: cdn` is an error.
  - `output_access_api::app_access_takes_keep_retention_and_signed_links`:
    - PUT `{"keep_outputs": false, "retention_days": 30, "signed_links": "litegen"}` → 200, with the overrides echoed and `effective` matching;
    - PUT `{"retention_days": 36501}` → 400 `invalid_retention`;
    - PUT `{"signed_links": "cdn"}` → 400 `invalid_signed_links`;
    - PUT `{"keep_outputs": "no"}` → 400 (`invalid_keep_outputs` or the JSON rejection, whichever the extractor gives; assert 4xx and no row change).
  - `output_access_api::gateway_settings_take_the_new_fields`:
    - PUT `/v1/admin/settings` with `storage_access_retention_days: 36501` → 400;
    - a valid PUT shows them in `effective_storage_access`.
  - A db test in both suites: `app_file_access_round_trips_the_new_columns`.
- [ ] **Step 2: Run them, expect FAIL.** Run `CARGO_INCREMENTAL=0 cargo test --lib access:: && cargo test --test output_access_api`. Expected: compile errors on the missing fields.
- [ ] **Step 3: Implement.** Add the migration, the fields and the layering in `effective()` (unknown stored values fall through), the validation in `put_app_access` and the settings PUT, and the audit details.
- [ ] **Step 4: Run them, expect PASS.** Also run `cargo test --lib management::` and `db::`.
- [ ] **Step 5: Ledger line** (no commit).

### Task 2: The keep decision, and images that keep nothing

**Files:**
- Modify: `litegen-core/src/access/mod.rs` (`keep_decision`), `litegen-core/src/api/handlers/mod.rs` (`generate_image`, `keep_image_result`), `litegen-core/src/proxy/router.rs` (`generate_image_cached` / `build_image_results`), `litegen-core/src/proxy/cache.rs` (`CachePolicy::bypass()`)
- Test: `litegen-core/tests/keep_nothing_api.rs` (new)

**Interfaces:**
- Produces:
  - `pub enum KeepHeader { Absent, KeepNothing }`.
  - `pub fn keep_header(headers: &HeaderMap) -> Result<KeepHeader, String>`. Only `false`, case-insensitive, is accepted.
  - `pub fn keeps_outputs(policy: &AccessPolicy, header: KeepHeader) -> bool`.
  - `CachePolicy::bypass()` leaves the cache untouched and answers `X-Litegen-Cache: bypass`.

- [ ] **Step 1: Write the failing tests** (mock provider, SQLite harness):
  - `an_app_that_keeps_nothing_gets_inline_images_and_no_copy`:
    - app access `keep_outputs: false`;
    - POST `mock/image` with `response_format: "url"` → 200, `data[0].b64_json` is set, `data[0].url` is null;
    - the generation row exists with `metadata.assets` empty or absent and `result_url` null;
    - the local asset dir holds no `image_0` file for that id.
  - `the_header_keeps_nothing_for_one_request`: with no app setting, `X-Litegen-Keep-Outputs: false` gives the same outcome, and the next request without the header writes the copy.
  - `the_header_takes_only_false`: `X-Litegen-Keep-Outputs: true` → 400 `invalid_keep_outputs_header`.
  - `keeping_nothing_bypasses_the_cache`: with the cache on and two identical requests, both carry `X-Litegen-Cache: bypass` and the mock is called twice.
  - `keeping_nothing_skips_an_s3_image_store`: the image store is a recording fake; with keep-nothing it records no `store` call.
- [ ] **Step 2: Run, expect FAIL.** Run `cargo test --test keep_nothing_api`. Expected: the assertions fail (a copy is written; the header is ignored).
- [ ] **Step 3: Implement.**
  - Parse the header first, so a bad value is a 400 before any vendor call.
  - Resolve the policy and decide.
  - When keeping nothing: pass a no-op image store, `CachePolicy::bypass()`, and `keep_image_result(…, keep=false)`, which records the row without writing files.
- [ ] **Step 4: Run, expect PASS.** Also run `cargo test --lib api::handlers`.
- [ ] **Step 5: Ledger line.**

### Task 3: Video that keeps nothing

**Files:**
- Modify: `litegen-core/src/types/mod.rs` (`GenerationAsset.kept: bool`, serde default `true`, serialized only when `false`), `litegen-core/src/proxy/router.rs` (`rehost_video` gains `keep: bool`; `get_video_status`), `litegen-core/src/proxy/poller.rs`, `litegen-core/src/api/handlers/mod.rs` (`generate_video` saves `metadata.keep_outputs`; `get_video_status` reads it from the row)
- Test: `tests/keep_nothing_api.rs`; `router.rs` unit tests

**Interfaces:**
- Consumes: Task 2's `keep_header` and `keeps_outputs`.
- Produces:
  - `rehost_video(store, prefix, generation_id, data, url, content_type, keep: bool)`. With `keep == false` it returns a `StoredVideo` whose asset is `{ url: vendor, kept: false, format: from the URL's extension or mp4 }` and stores nothing.
  - Bytes with no URL → `RehostError::Permanent("… keeps nothing …")`.
  - `pub fn row_keeps_outputs(metadata: Option<&serde_json::Value>) -> bool` (absent → `true`).

- [ ] **Step 1: Write the failing tests:**
  - `router::tests::keeping_nothing_hands_out_the_vendor_link_and_stores_nothing`: a recording store, `keep=false`, a URL → `Ok`, `asset.kept == false`, url unchanged, zero puts.
  - `router::tests::keeping_nothing_fails_a_bytes_only_video`: `data` set, no URL, `keep=false` → `Permanent`, and the message contains "keeps nothing".
  - `keep_nothing_api::a_video_that_keeps_nothing_answers_the_vendor_link`:
    - mock video, app keep-nothing, public mode;
    - poll until completed → `video_url` is the mock's vendor URL, and the row's `metadata.assets[0].kept == false`.
  - `keep_nothing_api::the_decision_is_taken_at_submit`: submit with keep-nothing, switch the app to keep, then complete → still `kept: false`. The reverse case (submit keeping, then switch) → kept.
  - `keep_nothing_api::the_poller_honours_a_saved_keep_nothing`: `poll_once` on a row with `metadata.keep_outputs: false` → `result_url` is the vendor link.
- [ ] **Step 2: Run, expect FAIL** (no `keep` parameter; the video is re-hosted).
- [ ] **Step 3: Implement.** Save `keep_outputs: false` in the row's metadata at submit (only when false). Both observers read it with `row_keeps_outputs`; the client poll path reads the row before calling `get_video_status`. Thread `keep` through.
- [ ] **Step 4: Run, expect PASS.** Also run `cargo test --lib proxy::`.
- [ ] **Step 5: Ledger line.**

### Task 4: 3D that keeps nothing

**Files:**
- Modify: `litegen-core/src/providers/mod.rs` (`Model3dFile.source_url: Option<String>`), `litegen-core/src/providers/model3d/fal.rs` (set it at the three `fetch_asset` sites), `litegen-core/src/proxy/router.rs` (`rehost_model3d_files` and its callers: `keep == false` records source URLs as `kept: false` assets with no synthesis), `litegen-core/src/api/handlers/mod.rs` (`generate_3d`: the format check and saving `keep_outputs`), the poller's model3d arm
- Test: `tests/keep_nothing_api.rs` (fal against wiremock, as `tests/model3d_*` do); `model3d/fal.rs` unit tests

**Interfaces:**
- Produces:
  - `pub fn keep_nothing_assets(files: &[Model3dFile]) -> Result<Vec<GenerationAsset>, String>`. It returns an error when a file has no `source_url`.
  - The native format list for a schema: `schema.output_formats.enum_values`.

- [ ] **Step 1: Write the failing tests:**
  - `fal::tests::delivered_files_carry_the_url_they_came_from`: the poll of a completed job sets `source_url` on its mesh and preview.
  - `keep_nothing_api::a_3d_request_for_a_converted_format_is_refused`: keep-nothing, a fal row with native `[glb]`, request `output_formats: ["obj"]` → 400 `format_needs_keeping`. The message lists `glb`, and no vendor call is made (wiremock expects 0 submits).
  - `keep_nothing_api::a_3d_job_that_keeps_nothing_hands_out_the_vendors_files`:
    - keep-nothing, request `[glb]`, completion → status completed;
    - `assets[0].url` is the wiremock mesh URL, with `kept: false`;
    - the store holds no file for the id.
  - `router::tests::a_file_without_a_source_url_fails_keeping_nothing`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.** Skip `synthesize_missing_formats` and the re-host when keeping nothing; the guard still runs on native formats.
- [ ] **Step 4: Run, expect PASS.** Also run `cargo test --test model3d_conformance` and `cargo test --lib providers::model3d`.
- [ ] **Step 5: Ledger line.**

### Task 5: litegen's signed file-route links, and streaming vendor files

**Files:**
- Modify: `litegen-core/src/access/signing.rs` (`sign_file_route`, `verify_file_route`), `litegen-core/src/access/present.rs` (signed mode: a litegen link when `signed_links: litegen` or when no store can sign), `litegen-core/src/api/handlers/mod.rs` (`get_generation_asset`: the signature path and kept=false streaming)
- Test: `access/signing.rs` unit tests; `tests/keep_nothing_api.rs`; `tests/output_access_api.rs`

**Interfaces:**
- Produces:
  - `LinkSigner::file_route_query(generation_id, index, issued: i64, expires: i64) -> String` (`expires=…&issued=…&signature=…`).
  - `LinkSigner::verify_file_route(generation_id, index, issued, expires, sig) -> bool`.
  - `pub enum LinkCheck { Valid, Invalid, Expired, Revoked }`. Revocation comes in Task 6; this task returns `Revoked` never.

- [ ] **Step 1: Write the failing tests:**
  - `signing::tests::a_file_route_link_verifies_only_for_its_generation_index_and_times`: changing any of id, index, issued or expires fails; it never verifies as a local-store key.
  - `output_access_api::a_litegen_signed_link_opens_without_credentials`:
    - signed mode, `signed_links: litegen`, local store;
    - the answer's url is `/v1/generations/{id}/assets/0?expires=…`;
    - a GET with no auth → 200 bytes, with nosniff and CSP sandbox;
    - a tampered signature → 403 `invalid_link`;
    - a past expiry → 403 `link_expired`.
  - `keep_nothing_api::a_private_app_streams_the_vendor_file`:
    - keep-nothing, private, the vendor URL on wiremock;
    - the answer's `video_url` is the file route, and it never contains the wiremock host;
    - GET with the key → 200 with the wiremock body, nosniff and CSP.
  - `keep_nothing_api::a_signed_app_gets_a_litegen_link_to_the_vendor_file`: keep-nothing and signed → a litegen link with `video_url_expires_at` set; GET with no auth → the wiremock body.
  - `keep_nothing_api::a_vendor_file_that_is_gone_answers_410`: wiremock answers 404 → 410 `vendor_file_gone`; a hung vendor is bounded by the client timeout (test with a 2 s delay and a test timeout of 1 s → 502 or 504, never a hang; assert it finishes under 5 s).
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
  - The vendor fetch is `check_public_url`, then the no-redirect client, capped at `MAX_VIDEO_BYTES`.
  - A 404 or 410 → `vendor_file_gone`.
  - Tests reach wiremock on 127.0.0.1 through the same test-only allowance the 3D fetch uses (`public_assets_only = false`). Read how `tests/model3d_*` do it, and mirror that rather than loosening the production check.
- [ ] **Step 4: Run, expect PASS.** Also run `cargo test --test output_access_api` and `cargo test --test known_gaps_api`.
- [ ] **Step 5: Ledger line.**

### Task 6: Revoking links

**Files:**
- Create: `migrations/{sqlite,postgres}/20240101000019_link_revocations.sql` (`link_revocations(scope TEXT, scope_id TEXT, revoked_at TIMESTAMP, PRIMARY KEY(scope, scope_id))`)
- Modify:
  - `litegen-core/src/db/{trait_def,sqlite,postgres}.rs`: `revoke_links(scope, scope_id, at)` and `links_revoked_at(app_id, generation_id) -> Option<DateTime>`, the latest of the gateway, app and generation rows;
  - `litegen-core/src/access/signing.rs`: local-store links gain `issued`;
  - `litegen-core/src/proxy/storage.rs` (`LocalModel3dStorage::signed_url`), `litegen-core/src/api/handlers/mod.rs` (`get_model3d_asset_bytes`, `get_generation_asset`), `litegen-core/src/api/handlers/orgs.rs`;
  - the routes: `POST /v1/apps/{app_id}/access/revoke-links`, `POST /v1/generations/{id}/revoke-links` and `POST /v1/admin/revoke-links`;
  - `litegen-core/src/auth/permissions.rs`: `GenerationDeleteOwn` and `GenerationDeleteAny`, added wherever the cancel permissions are.
- Test: `tests/output_access_api.rs`; both db suites

**Interfaces:**
- Produces:
  - `RevocationScope { Gateway, App, Generation }`.
  - The answer `{ "revoked_at": "<rfc3339>" }`.
  - `AppAccessSettings.links_revoked_at: Option<DateTime>`.

- [ ] **Step 1: Write the failing tests:**
  - `output_access_api::revoking_an_apps_links_ends_them_at_once`:
    - a litegen link and a local-store link both work;
    - POST revoke for the app → both 403 `link_revoked`;
    - a link minted after → works.
  - `output_access_api::revoking_one_generations_links_spares_the_others`.
  - `output_access_api::the_gateway_revocation_ends_every_litegen_link`.
  - `output_access_api::a_revocation_reaches_another_replica`: two app states on one DB file; revoke on A, verify on B → 403. This covers Review Focus 3.
  - `output_access_api::a_link_without_issued_dies_at_the_first_revocation`: a legacy-format local link works until a revocation.
  - `output_access_api::revoking_needs_the_right_permission`: a member without the storage permission → 403; a viewer can't revoke a generation.
  - db: `link_revocations_take_the_latest_scope`.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.** Audit actions: `app.access.revoke_links`, `generation.revoke_links` and `gateway.revoke_links`.
- [ ] **Step 4: Run, expect PASS.** Also run the db suites and `cargo test --lib auth::`.
- [ ] **Step 5: Ledger line.**

### Task 7: Retention

**Files:**
- Create: `migrations/{sqlite,postgres}/20240101000020_files_deleted_at.sql`. It adds `generations.files_deleted_at TIMESTAMP NULL`, an index on `(app_id, created_at)` if one is missing, and a `files_deleted_reason TEXT NULL`.
- Modify:
  - `litegen-core/src/db/*`:
    - `generations_due_for_retention(app_id: Option<&str>, exclude_apps: &[String], cutoff, limit) -> Vec<Generation>`;
    - `mark_files_deleted(id, at, reason)`;
  - `litegen-core/src/types/mod.rs`: `Generation.files_deleted_at`;
  - new `litegen-core/src/proxy/retention.rs`, holding `sweep_retention_once(...) -> SweepReport` and `spawn_retention_sweep`;
  - `litegen-core/src/main.rs` or the server bootstrap, which spawns the sweep;
  - `get_generation_asset`, which answers 410.
- Test: `tests/retention_api.rs` (new); both db suites

**Interfaces:**
- Produces:
  - `pub async fn delete_generation_files(stores: &[Arc<dyn ImageStorage>], gen: &Generation) -> Result<usize, String>`. Task 8 reuses it.
  - `pub const RETENTION_BATCH: usize = 500`.

- [ ] **Step 1: Write the failing tests:**
  - `retention_api::files_older_than_the_apps_retention_are_deleted`:
    - app retention 1 day;
    - one generation with created_at 2 days ago and one 1 hour ago;
    - `sweep_retention_once` → the old one's file is gone, `files_deleted_at` is set and the reason is `retention`;
    - the new one is untouched;
    - the file route → 410 `files_deleted`.
  - `retention_api::retention_zero_keeps_everything`.
  - `retention_api::an_app_override_beats_the_gateway_default`: gateway 30 and app 0 → nothing swept; gateway 1 and an app without an override → swept.
  - `retention_api::a_failed_delete_is_retried_next_sweep`: the store fails once → the row is not marked; the next sweep deletes and marks it.
  - `retention_api::a_backlog_is_swept_in_batches`: `limit = 2` and 5 due rows → 2 marked, then 4, then 5. This covers Review Focus 4.
  - `retention_api::kept_false_assets_are_not_deleted_anywhere`: a vendor link isn't deleted; the row is marked.
  - db: `generations_due_for_retention_respects_app_cutoffs` (in both suites).
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
  - Delete first and mark second.
  - Sweep apps with an override one at a time, then the default pass excluding them.
  - Run hourly with the first tick after 5 minutes, so tests and boot stay quiet.
- [ ] **Step 4: Run, expect PASS.** Also run the db suites.
- [ ] **Step 5: Ledger line.**

### Task 8: Deleting generations, apps' and orgs' files

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs` (`delete_generation`, route `DELETE /v1/generations/{id}`), `litegen-core/src/api/handlers/orgs.rs` (`delete_app`, `delete_org`), `litegen-core/src/db/*` (`redact_generation_request(id)`, `list_generation_assets_for_app/org`)
- Test: `tests/retention_api.rs`

**Interfaces:**
- Consumes: Task 7's `delete_generation_files` and `mark_files_deleted`; Task 6's permissions.

- [ ] **Step 1: Write the failing tests:**
  - `retention_api::deleting_a_generation_deletes_its_files_and_request`: DELETE → 200; `files_deleted_at` is set with reason `deleted`; `metadata.request` is absent; the file is gone; the cost stays.
  - `retention_api::a_running_generation_is_not_deleted`: a pending mock video → 409 `generation_running`.
  - `retention_api::deleting_needs_the_delete_permission`: a key of another app (owner scope `key`) → 403/404 as the read path does; a member with only read → 403.
  - `retention_api::deleting_an_app_deletes_its_files_in_litegens_store`: an app with files in the local store → after `DELETE /v1/apps/{id}` (and the background task finishes; await the join handle the handler returns in tests, or poll for up to 5 s) its files are gone.
  - `retention_api::deleting_an_app_leaves_its_own_bucket_alone`: an app with a BYO recording store → no delete calls.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Ledger line.**

### Task 9: App buckets through an IAM role, and the fallback choice

**Files:**
- Create: `litegen-core/src/proxy/aws_sts.rs`. It holds the SigV4 signer, `assume_role`, `get_caller_identity` and the credential cache.
- Create: `migrations/{sqlite,postgres}/20240101000021_app_storage_roles.sql`. It adds `credential_mode TEXT NULL`, `role_arn TEXT NULL` and `fallback_to_litegen_store BOOLEAN NULL` to the app storage table (name it as the existing migration does).
- Modify:
  - `litegen-core/src/config/mod.rs`: `AwsConfig`, and `S3StorageConfig.session_token`;
  - `litegen-core/src/proxy/storage.rs`: the session token passed to rust-s3, and `signed_url` refusing a presign that outlives the credentials;
  - `litegen-core/src/api/handlers/mod.rs`: `resolve_app_image_store`, `resolve_app_model3d_store`, the fallback;
  - `litegen-core/src/api/handlers/orgs.rs`: put/get app storage;
  - `litegen-core/src/types/mod.rs`: the storage row, upsert and info types.
- Test: `aws_sts.rs` unit tests (the SigV4 example vector); `tests/app_storage_roles.rs` (new; wiremock STS and wiremock S3)

**Interfaces:**
- Produces:
  - `pub fn external_id(secrets_key: &[u8; 32], app_id: &str) -> String` (`litegen-` plus 32 hex).
  - `pub async fn assumed_credentials(cfg: &AwsConfig, role_arn: &str, external_id: &str, session: &str) -> Result<TempCredentials, StsError>`, cached.
  - `TempCredentials { access_key_id, secret_access_key, session_token, expiration }`.
  - `AppStorageInfo` gains `credential_mode`, `role_arn`, `external_id`, `litegen_principal_arn` and `fallback_to_litegen_store`.

- [ ] **Step 1: Write the failing tests:**
  - `aws_sts::tests::sigv4_matches_the_aws_example`: AWS's documented IAM `ListUsers` GET example (AKIDEXAMPLE, 20150830T123600Z) gives the documented signature. If the remembered constant is wrong, cross-check the signing key against `s3::signing::signing_key` for service `s3`, and assert equality with it instead. Record that as a ruling.
  - `aws_sts::tests::the_external_id_is_stable_per_app_and_differs_between_apps`.
  - `app_storage_roles::a_role_bucket_signs_with_the_assumed_credentials`:
    - wiremock STS answers an AssumeRole XML (ASIA… key, a session token, expiration +1 h);
    - PUT storage `{credential_mode: assume_role, role_arn}` → 200, and STS got `RoleArn`, `ExternalId` = the app's, and a SigV4 `Authorization` with litegen's key;
    - an image generation copies to a wiremock S3 whose PUT carries `x-amz-security-token` and `Credential=ASIA…`.
  - `app_storage_roles::a_failing_role_answers_400_on_save`: STS 403 → 400 `assume_role_failed`, carrying STS's message.
  - `app_storage_roles::a_bad_role_arn_is_refused`: 400 `invalid_role_arn`.
  - `app_storage_roles::the_get_shows_the_external_id_before_a_role_is_saved`.
  - `app_storage_roles::cached_credentials_are_refreshed_before_they_expire`: expiration +4 min → a second use calls STS again; +1 h → it doesn't. This covers Review Focus 5.
  - `app_storage_roles::a_presign_longer_than_the_session_becomes_a_litegen_link`: signed mode, TTL 2 h, session 1 h → a litegen link.
  - `app_storage_roles::without_fallback_a_broken_bucket_keeps_nothing_in_litegens_store`:
    - `fallback_to_litegen_store: false` and STS failing;
    - the image answer is still 200;
    - no file appears in the local store;
    - a video's keep step is transient (the row stays processing).
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
  - Use `aws-creds` `Credentials::default()` on `spawn_blocking` when `AwsConfig` has no keys.
  - Parse the STS XML with `quick_xml`.
  - Map an STS error body's `<Message>` into `assume_role_failed`.
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Ledger line.**

### Task 10: The storage bugs (backend)

**Files:**
- Modify: `litegen-core/src/api/handlers/mod.rs` (the image cache-hit presenting; `list_generations` with presenters memoized by app)
- Test: `tests/gateway_settings_api.rs` or `tests/output_access_api.rs`

- [ ] **Step 1: Write the failing tests:**
  - `output_access_api::a_cached_image_answer_in_a_private_app_uses_the_file_route`:
    - private app with a recording S3 image store, cache on;
    - the second identical request is a `hit`, and its `data[0].url` is `/v1/generations/{original}/assets/0`, never the bucket URL.
  - `output_access_api::listing_resolves_each_apps_policy_once`: a counting DB wrapper or a policy-lookup counter; 10 rows of one app → 1 lookup.
- [ ] **Step 2: Run, expect FAIL.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS.**
- [ ] **Step 5: Ledger line.**

### Task 11: Dashboard

**Files:**
- Modify: `dashboard/src/pages/Organization.tsx` (the access and storage panels), `dashboard/src/pages/Gateway.tsx` (Stored files), the generation detail view (find it; it is likely `Logs.tsx` or `TracePanel.tsx`), `dashboard/src/playground/ResultTile.tsx` and `GenerationOutput.tsx` (GIF by `format`), the app switcher's stale access panel
- Test: vitest next to each, and a Playwright spec `dashboard/e2e/storage-flexibility.spec.ts`, plus whatever config the access suite uses

- [ ] **Step 1: Write the failing tests:**
  - vitest: the access panel sends `keep_outputs`, `retention_days` and `signed_links`; "Revoke links" posts and shows the time; it clears when the active app clears.
  - vitest: the storage form in role mode shows the external ID and sends `credential_mode` and `role_arn`.
  - vitest: a video asset with `format: "gif"` and an extensionless URL renders `<img>`.
  - Playwright `storage-flexibility.spec.ts`:
    - set the app to keep nothing, generate a mock image in the Playground, and see it rendered;
    - the generation's detail shows no stored file;
    - "Revoke links" works.
- [ ] **Step 2: Run, expect FAIL** (`npx vitest run …`).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run, expect PASS:** `npx vitest run`, `npx eslint .`, `npx tsc -b`, and the Playwright spec on the debug binary.
- [ ] **Step 5: Ledger line.**

### Task 12: OpenAPI, SDKs, docs

**Files:**
- Modify:
  - `litegen-core/src/api/openapi.rs`: the new paths and schemas, after pinging litegen-20;
  - `sdks/typescript/**` and `sdks/python/**`: methods for revoke (app, generation, gateway), generation delete, and the new fields;
  - regenerate with `sdks/scripts/regen-all.sh`;
  - `apps/landing/public/openapi.json`, if the regen writes it;
  - the docs: `docs/storage-access.md`, `docs/gateway-settings.md`, the README's BYO storage section, `.claude/skills/provider-standards/references/known-gaps.md`, and every skill that describes keeping (grep "re-host" and "keeps every result").
- Test: `cargo test --lib api::openapi`, the SDK contract tests (`npm test` in `sdks/typescript`, `pytest` in `sdks/python`)

- [ ] **Step 1:** Ping litegen-20; add the openapi entries. The openapi test fails until every route is listed. Run it.
- [ ] **Step 2:** Regenerate the SDKs; add the methods; run the contract tests → PASS.
- [ ] **Step 3:** Docs. In known-gaps.md, delete the four storage entries and the "Not covered yet" block of storage-access.md; add whatever stays open.
- [ ] **Step 4: Ledger line.**

### Task 13: Proof against a real S3 server

- [ ] **Step 1:** If no peer cargo is running: `CARGO_TARGET_DIR=<scratchpad>/s3s-target cargo install s3s-fs --root <scratchpad>/s3s` (time-box it to 15 minutes).
- [ ] **Step 2:** If it runs (`s3s-fs --host 127.0.0.1 --port 9555 --access-key minioadmin --secret-key minioadmin <dir>`; check its `--help` for the exact flags), run `LITEGEN_S3_TEST_ENDPOINT=http://127.0.0.1:9555 cargo test --test output_access_s3`.
  - Add `retention_and_delete_reach_a_real_bucket` to `output_access_s3.rs`. It is gated the same way: delete a generation and check the object 404s through the bucket API.
- [ ] **Step 3:** Ledger the result, or why it could not run.

### Final

- [ ] The full gate: `cargo test --no-fail-fast` with Postgres; dashboard vitest, eslint and tsc; the browser suites (main, multitenant, hooks, access, storage-flexibility) on the debug binary; the SDK tests.
- [ ] A self-review against code-reviewer.md. Fix the Critical and Important findings (RED→GREEN) and ledger the minors.
