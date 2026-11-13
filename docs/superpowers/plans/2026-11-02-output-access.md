# Stored-output access Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Who may open a stored output becomes a setting (`public` | `signed` | `private`, plus an owner scope), with YAML defaults and per-app overrides, enforced where files are served and applied to every URL litegen hands out. A `file_access` hook narrows it further.

**Architecture:**
- **Enforcement** happens where bytes leave litegen:
  - the local asset route checks a link signature or the app's mode;
  - the private file route checks the owner scope, then the hook;
  - the bucket itself must be private (documented).
- **Presentation** happens at answer time. One presenter rewrites each stored URL into the caller's form: unchanged, signed, or the private route.
- **Settings resolve** as the app's override, else the YAML default.

**Tech Stack:** Rust (axum, sqlx sqlite and postgres, rust-s3 0.35 `presign_get`, hmac and sha2), the hook runners (Python, Node, Bun), the TS SDK, the React dashboard, Playwright.

**Spec:** `docs/superpowers/specs/2026-11-02-output-access-design.md`

## Global Constraints

- Default `public`: a deployment with no settings answers byte-for-byte as today, and the existing suites pass unchanged.
- `signed_url_ttl_seconds` is 60–604800, default 3600. A presigned S3 GET can't exceed 7 days.
- Owner scope `key` | `app` | `org`, default `key`.
- The master key, `generation:read:any` sessions, and `generation:read:own` sessions (for their own keys' files) always pass.
- No end-user isolation by default.
- `file_access` only narrows. It runs after the owner check; a failing hook refuses (503) unless `on_error: allow`.
- Shared checkout:
  - no commits without the user's go;
  - `CARGO_INCREMENTAL=0`, one cargo at a time;
  - ping the peer session before editing `litegen-core/src/api/openapi.rs`;
  - never touch `src/mesh/`, and never commit `scripts/model-drift/snapshots/`.
- No subagents.

## Review Focus

1. **A key outside the scope reads a generation's JSON in `signed` mode.** It must not get a working link: its URLs are the private route, which refuses it. Tested in Task 4.
2. **Unsigned or tampered local links in non-public modes.** An unsigned fetch, a link signed for another key, a changed `expires`, or an expired link must not serve. Tested in Task 3.
3. **Index drift.** A private-route URL must name the same file `GET …/assets/{index}` streams, including image results where a kept asset was skipped. Tested in Task 4.
4. **An app switches modes after files exist.** The new mode applies to old files at the next read; nothing is re-uploaded. Tested in Task 4.
5. **A hook file defining only `file_access`** must start. The runners' "defines neither…" check covers all three functions. Tested in Task 6.

---

### Task 1: Access settings (config, table, resolution)

**Files:**
- Create:
  - `litegen-core/src/access/mod.rs` (`AccessMode`, `OwnerScope`, `AccessPolicy`, `effective`, `resolve_policy`);
  - `litegen-core/migrations/{sqlite,postgres}/20240101000017_app_file_access.sql`.
- Modify:
  - `src/config/mod.rs`: `storage_access: StorageAccessConfig`, validated at load;
  - `src/lib.rs`;
  - `src/db/trait_def.rs`, `sqlite.rs`, `postgres.rs`, `sqlite_tests.rs`, `postgres_tests.rs`.

**Interfaces (produces):**
- `enum AccessMode { Public, Signed, Private }` and `enum OwnerScope { Key, App, Org }`. Both serde lowercase, `as_str()`, `FromStr`.
- `struct StorageAccessConfig { mode: AccessMode, owner_scope: OwnerScope, signed_url_ttl_seconds: u32 }`, with `deny_unknown_fields` and defaults public/key/3600.
- `struct AccessPolicy { mode, owner_scope, ttl: Duration }`.
- `fn effective(defaults: &StorageAccessConfig, row: Option<&AppFileAccessRow>) -> AccessPolicy`.
- `async fn resolve_policy(state: &AppState, app_id: Option<&str>) -> AccessPolicy`. A DB error logs and falls back to the defaults; an unknown stored value is ignored per field.
- `struct AppFileAccessRow { app_id, mode: Option<String>, owner_scope: Option<String>, signed_url_ttl_seconds: Option<i64>, updated_at }`.
- DB trait methods:
  - `get_app_file_access(&str) -> Option<AppFileAccessRow>`;
  - `put_app_file_access(&AppFileAccessRow)`, an upsert;
  - `delete_app_file_access(&str) -> bool`.

  The defaults are `Ok(None)` / `Ok(())` / `Ok(false)`, like `app_storage`.
- The table is `app_file_access(app_id PK REFERENCES apps(id) ON DELETE CASCADE, mode, owner_scope, signed_url_ttl_seconds, updated_at)`.

- [ ] **Step 1: Tests**
  - Config unit tests in `config/mod.rs`:
    - `storage_access_defaults_to_public_key_and_an_hour`;
    - `storage_access_refuses_a_ttl_outside_a_minute_to_a_week` (59 and 604801 are refused with a message naming the field);
    - `storage_access_refuses_unknown_fields`.
  - `access/mod.rs` unit tests:
    - `an_app_override_wins_field_by_field` (mode from the row, the scope from YAML);
    - `an_unknown_stored_value_falls_back_to_the_default`.
  - The DB round trip, `app_file_access_round_trips_and_cascades_with_the_app`, in `sqlite_tests.rs` and `postgres_tests.rs`: put, get, overwrite, delete; deleting the app removes the row.
- [ ] **Step 2: Run them; expect compile failures** (the types don't exist yet).
- [ ] **Step 3: Implement.** Validate the TTL where the other config checks run (`AppConfig` load), so a bad value stops startup with the field named.
- [ ] **Step 4: Run them.** Run `cargo test --lib access:: config::` and the sqlite DB tests. Expected: pass. Then run the Postgres test against the Docker container.

### Task 2: Link signing in the stores

**Files:**
- Create: `litegen-core/src/access/signing.rs`.
- Modify:
  - `src/proxy/storage.rs`: `signed_url` on `ImageStorage`; S3 presign; local HMAC; S3Store builds its URLs through S3Storage's `object_url` logic;
  - `src/proxy/router.rs` and `src/main.rs`: build the signer once and hand it to the local store;
  - `src/api/handlers/mod.rs` (`resolve_app_model3d_store` is unchanged: app buckets are S3).

**Interfaces (produces):**
- `struct LinkSigner([u8; 32])`:
  - `from_secrets_key(Option<&str>) -> LinkSigner`: the HMAC of the secrets key with label `litegen-asset-links-v1`, else random;
  - `sign(&self, key: &str, expires: i64) -> String` (hex);
  - `verify(&self, key, expires, sig) -> bool`, constant-time.
- `ImageStorage::signed_url(&self, key: &str, ttl: Duration) -> Option<(String, DateTime<Utc>)>`, default `None`.
  - S3Storage: `bucket.presign_get(key, ttl_secs, None)`, always on the bucket's own endpoint, never `custom_public_url`.
  - LocalModel3dStorage: `{base}/v1/models3d/assets/{key}?expires={unix}&signature={hex}`.
- `LocalModel3dStorage::new(root, base, signer: Arc<LinkSigner>)`, so every constructor call site changes.
- `build_model3d_store(config, base, signer)`.

- [ ] **Step 1: Tests**
  - `signing.rs`:
    - `a_link_verifies_only_for_its_key_and_expiry`: another key, another expiry, a changed signature and a truncated signature all fail;
    - `the_secrets_key_makes_links_survive_a_restart`: the same secrets key gives the same signature, and none gives different signers.
  - `storage.rs`:
    - `an_s3_store_presigns_on_the_bucket_endpoint_with_the_ttl`: `X-Amz-Expires=900` and `X-Amz-Signature=`, the path holds the key, and the host is the endpoint even with a `custom_public_url`;
    - `a_local_store_signs_its_own_route`;
    - `the_image_store_mints_the_same_url_as_the_asset_store`. Path-style MinIO, AWS, and `custom_public_url` all give equal URLs, and `S3Storage::key_of` recovers the key.
- [ ] **Step 2: Run; expect failures.**
- [ ] **Step 3: Implement.** Factor `object_url` into a free function both S3 types call.
- [ ] **Step 4: Run** `cargo test --lib proxy::storage access::signing`. Expected: pass.

### Task 3: The local asset route enforces the mode

**Files:**
- Modify: `get_model3d_asset_bytes` in `src/api/handlers/mod.rs`.
- Test: `litegen-core/tests/output_access_api.rs` (new, with the harness from `tests/harness`).

**Behaviour:**
- **A request with `expires` and `signature`:**
  - a valid, unexpired link is served with `Cache-Control: private, max-age=<remaining, at most 3600>`;
  - a valid but expired link → 403, `{"error":{"code":"link_expired"}}`;
  - an invalid link → 404.
- **A request without them:** the generation id is the key segment after the prefix. Its row gives the app, and the app gives the policy (a missing row uses the YAML defaults).
  - `public` → served as today (`public, max-age=31536000, immutable`);
  - otherwise → 404.

**Tests** (named, in `output_access_api.rs`):
- `a_public_app_serves_its_files_unsigned_as_before`;
- `a_signed_app_serves_a_valid_link_and_nothing_unsigned`;
- `an_expired_link_answers_link_expired`. Mint with a 1 s expiry through the store, or craft one with the signer and a past `expires`;
- `a_tampered_link_is_not_found`: the key, `expires` or `signature` edited;
- `a_private_app_serves_nothing_from_the_asset_route`.

Steps: write the tests → run (fail) → implement → run (pass).

### Task 4: Presenting URLs everywhere they leave litegen

**Files:**
- Create: `litegen-core/src/access/present.rs`.
- Modify:
  - `src/types/mod.rs`: `expires_at: Option<DateTime<Utc>>` on `ImageResult`, `Model3dAsset` and `GenerationAsset`, and `video_url_expires_at` on `VideoGenerationResponse`, all skipped when none;
  - in handlers: `generate_image`, `generate_video`, `get_video_status`, `generate_3d`, `get_3d_status`, `get_generation`, `list_generations`, `list_generation_assets`;
  - `keep_image_result`, which returns the assets it recorded;
  - `dispatch_terminal_webhook` in `proxy/poller.rs`, which gets the presenter's inputs.

**Interfaces (produces):**
- `struct Presenter { policy: AccessPolicy, in_scope: bool, stores: Vec<Arc<dyn ImageStorage>>, base_url: String, generation_id: String }`.
- `Presenter::url(&self, index: Option<u32>, stored: &str) -> (String, Option<DateTime<Utc>>)`:
  - `public` → unchanged;
  - `private`, or `signed` when the caller is out of scope → `{base}/v1/generations/{id}/assets/{index}` (unchanged when there is no index);
  - `signed` and in scope → the first store whose `key_of` matches signs it. If none does → the private route.
- `async fn presenter_for(state, viewer: Option<&KeyContext>, gen_id, app_id, gen_key_id, gen_org_id) -> Presenter`.
- `fn present_row(p: &Presenter, gen: &mut Generation)`: indexes come from `generation_assets(gen)` by stored URL, rewriting `metadata.assets[*].url`/`expires_at` and `result_url` (+`result_url_expires_at` in metadata).
- In-scope rule, `in_owner_scope(scope, viewer, gen_key_id, gen_app_id, gen_org_id) -> bool`:
  - no viewer, the platform admin, or a session → true;
  - a key: `Key` compares key ids, `App` app ids, `Org` org ids.
- Webhook viewer: the generation's own key (always in scope).

**Tests** (in `output_access_api.rs`, with the mock providers):
- `public_answers_are_unchanged`: the image, video and 3D answers and generation reads carry the stored URLs and no `expires_at`;
- `a_signed_app_answers_links_that_work_without_credentials`: the 3D status, `GET /v1/generations/{id}` (`metadata.assets` and `result_url`), the asset list and the video status each carry signed links and `expires_at`. Each link fetched with no `Authorization` gives 200 and the stored bytes;
- `a_private_app_answers_the_file_route`, with every index matching the streamed file. This includes an image result whose kept assets skipped one;
- `a_key_outside_the_scope_gets_no_working_link_in_signed_mode`: another key of the same org reads the generation, its URLs are the private route, and that route answers 403;
- `switching_an_app_to_private_applies_to_files_made_before`;
- `the_webhook_carries_links_in_the_apps_mode`, with a wiremock webhook receiver and a signed app: the payload's `result_url` is signed and fetches 200.

Steps: write the tests → run (fail) → implement → run the new tests plus `cargo test --test api_integration` and the e2e middleware tests, which must stay green unchanged (public default).

### Task 5: The owner scope on the file route

**Files:**
- Modify: `get_generation_asset` in `src/api/handlers/mod.rs`.

**Behaviour:** after `readable_generation`, the caller must be `in_owner_scope` under the app's policy. If not → 403:

```json
{"error":{"type":"forbidden","code":"not_file_owner","message":"this file belongs to another key; the app's owner scope is key"}}
```

The message names the scope. This applies in every mode, since the route is litegen's own file route.

**Tests:**
- `the_file_route_follows_the_owner_scope`, a matrix: keys A1 and A2 on app A, key B1 on app B, all in one org.
  - scope `key`: A1 → 200, A2 → 403, B1 → 403;
  - scope `app`: A2 → 200, B1 → 403;
  - scope `org`: B1 → 200;
  - the master key → 200;
  - another org's key → 404.
- `sessions_keep_their_read_permissions_on_files`: a read:any session → 200; read:own → 200 for its own key's file and 403 otherwise.

Steps: tests → fail → implement → pass.

### Task 6: End users recorded, the `file_access` hook, the file route hooked

**Files:**
- Modify:
  - `src/hooks/process.rs`, `HookFn::FileAccess` (`"file_access"`);
  - `src/hooks/config.rs`, `file_policy` (default Refuse);
  - `src/hooks/layer.rs`, `failed` for the third phase;
  - `runners/runner.py` and `runner.mjs` (`HOOKS` gains `file_access`; the error becomes "defines none of pre_call, post_call, file_access");
  - `src/hooks/mod.rs`, `pub async fn file_access(hooks, request_view, file, caller) -> Result<(), Response>`;
  - the handlers: `generate_image`/`keep_image_result`, `generate_video` and `generate_3d` store `metadata.end_user` from the `RequestAttribution` they already take;
  - `get_generation_asset` calls the hook;
  - `create_router_with_hooks`: the file route moves into a hooked group with `Scope::Read` and CSRF, plus `Extension(hooks)`.
- Fixtures: the `tests/fixtures/hooks/behaviour.{py,mjs,ts}` files gain `file_access`, which reads `x-test-file` actions:
  - `deny-other-end-user`: refuse 403 unless `file.created_by.end_user == request.headers['x-litegen-end-user']`;
  - `report`: refuse 418 with the JSON of `file` and `caller`;
  - `fail`: raise;
  - `deny-string`: return a string.

  Add `file_only.py`, which defines only `file_access`.

**The hook contract:** `file_access(request, file, caller)`.
- `file = {generation_id, index, kind, format, size_bytes, media_type, model, created_at, created_by: {key_id, app_id, org_id, end_user}}`.
- Return nothing (or `true`) to serve.
- `false` → 403 `refused_by_hook`; a string → 403 with that message; a raised status → that status.
- Any other value is malformed and handled by the `on_error` policy.

**Tests:**
- `hooks_runtime.rs`: `a_file_access_only_hook_starts_in_every_runtime`.
- `hooks_api.rs`, per runtime (Python, Node, Bun, and Bun-TS):
  - `file_access_sees_the_file_and_the_caller` (`report`: generation id, index, `created_by.end_user` as recorded, `caller.key_id`);
  - `file_access_can_refuse_another_end_users_file`;
  - `a_failing_file_access_refuses_unless_allowed`;
  - `file_access_cannot_open_a_file_the_scope_refuses`: the hook allows everything; an out-of-scope key still gets 403, and the hook never ran (its `report` would have answered 418);
  - `pre_call_custom_auth_reaches_the_file_route`: no `Authorization`; the hook injects a key; 200.
- `output_access_api.rs`: `the_end_user_is_recorded_on_each_generation`, for image, video and 3D (`metadata.end_user`).

Steps: tests → fail → implement → pass. Then run the whole hooks suite, `hooks_runtime` + `hooks_api` + `hooks_e2e`.

### Task 7: The settings API, OpenAPI, SDKs

**Files:**
- Modify:
  - `src/api/handlers/orgs.rs`: `get_app_access`, `put_app_access`;
  - `create_router_with_hooks`, the route `/v1/apps/{app_id}/access`;
  - `src/api/openapi.rs`, **after pinging the peer session**: the paths and the schemas `AppAccessSettings` and `PutAppAccessRequest`, plus the new `expires_at` fields;
  - `sdks/` (regen via `sdks/scripts/regen-all.sh`), `sdks/typescript/src/client.ts` (`AppAccessNamespace` on `apps.access`: `get`, `put`), and its test.

**Interfaces:**
- `GET` returns:
  ```json
  {"app_id": "…",
   "overrides": {"mode": null, "owner_scope": null, "signed_url_ttl_seconds": null},
   "effective": {"mode": "…", "owner_scope": "…", "signed_url_ttl_seconds": 0},
   "updated_at": null}
  ```
- `PUT` takes the `overrides` shape. All fields are optional; null or absent means "use the YAML". An all-null body deletes the row. Answers 200 with the `GET` shape.
- **Permissions:**
  - `GET`: `Permission::StorageCredRead`, the same as app storage, through `require_member_perm`;
  - `PUT`: `StorageCredWrite`, and it writes an audit entry `app.access.update`.
- **Validation** → 400:
  - `invalid_mode`;
  - `invalid_owner_scope`;
  - `invalid_ttl` (outside 60–604800).

**Tests:**
- `multitenant_api.rs`:
  - `app_access_settings_round_trip_and_show_effective_values`: YAML signed; put private; get effective private; put nulls; get effective signed;
  - `app_access_settings_need_storage_permissions` (a viewer gets 403);
  - `app_access_settings_refuse_bad_values`.
- The OpenAPI sync check (whatever the repo runs: `openapi_spec_is_in_sync`, or the regen diff) passes after regen.
- The TS SDK test for `apps.access`.

Steps: tests → fail → implement → **ping the peer** → edit openapi.rs → regen → SDK facade → run the API, OpenAPI and SDK tests.

### Task 8: The dashboard panel

**Files:**
- Modify: `dashboard/src/pages/Organization.tsx`, adding an "Access to stored files" section under storage:
  - a mode select: "Use default (…)", public, signed, private;
  - an owner-scope select;
  - a TTL input (seconds);
  - Save;
  - the effective line;
  - a warning: "signed and private need a private bucket".
- Test: a dashboard e2e spec, `dashboard/e2e/output-access.spec.ts`, against the real binary like the other specs.

**Test:** `sets an app to signed and the next generation's files follow it`. It sets signed with a 600 s TTL in the panel, sees the effective line, generates a mock 3D model through the API with the app's key, and reads the generation: the asset URL carries `signature=`. The asset fetches with no credentials, and the unsigned URL gives 404.

Steps: spec → fail → implement → pass. Then the whole main dashboard e2e suite, and dashboard lint, typecheck and unit tests.

### Task 9: Docs, the YAML example, a real-bucket proof

**Files:**
- Create:
  - `docs/storage-access.md`: the modes, the use-case table, the settings (YAML and app), the bucket policy for signed and private (Block Public Access), the hook with tested examples quoted under `<!-- example: … -->` markers, caveats (links already handed out, CDN caches, idempotent replays, `custom_public_url` vs signed), and the deferred items;
  - `examples/hooks/file_access_end_user.py` and `file_access_end_user.mjs`;
  - `litegen-core/tests/output_access_s3.rs`, which no-ops without `LITEGEN_S3_TEST_ENDPOINT`. Run against a throwaway `minio/minio` container: a private bucket refuses the stored URL unsigned (403), and the signed link litegen answers fetches 200.
- Modify:
  - `README.md` (a "Stored outputs" section);
  - `litegen.example.yaml` (a `storage_access:` block recommending `signed`);
  - `docs/hooks.md` (`file_access`; the file route is hooked);
  - `.claude/skills/provider-standards/references/known-gaps.md` (the access gap closes; the deferred items are listed).
- Tests:
  - the guide test (as for `docs/hooks.md`), which runs the quoted examples against litegen per runtime;
  - the real-binary e2e in `hooks_e2e.rs` or `output_access_e2e.rs`: `CARGO_BIN_EXE_litegen` with `storage_access.mode: signed`. A 3D generation's link fetches over real HTTP without a key.

Steps: write the examples and tests → fail → write the docs and examples → pass. Then the full gate (`gate.sh` with Postgres), the hooks browser suite, and the main dashboard e2e.

## Final

A self-review against `code-reviewer.md` (no subagents, by the user's rule), the ledger, the fix pass, and the finishing skill. **No commit without the user's go.**
