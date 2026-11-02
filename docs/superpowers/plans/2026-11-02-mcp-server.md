# LiteGen MCP server (plan 1 of 2: server, API-key auth, docs) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. The user chose native execution with no subagents (2026-11-02): implement, review and verify in the session itself. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve LiteGen's AI API at `/mcp` and its management API at `/admin/mcp` as MCP, with exactly the HTTP API's permissions, for API-key and dev-mode callers, plus the docs for every client that can send a header.

**Architecture:** `litegen-core/src/mcp/` mounts two rmcp `StreamableHttpService`s (stateless) behind an axum auth layer. `tools/list` comes from the in-process OpenAPI document filtered through a curated catalog and per-tool gates. `tools/call` replays each call as an internal HTTP request through the real API router (`tower::ServiceExt::oneshot`), so every existing check runs unchanged. OAuth is plan 2 (`2026-11-02-mcp-oauth.md`).

**Tech Stack:** Rust 2021, axum 0.8, rmcp 3.8 (`server`, `transport-streamable-http-server`), utoipa 5, sqlx (SQLite/Postgres), `image` 0.25, Node 24 + `@modelcontextprotocol/sdk` for the smoke script.

**Spec:** `docs/superpowers/specs/2026-11-02-mcp-server-design.md`. Research: `docs/superpowers/research/2026-11-02-mcp-clients.md`, `docs/superpowers/research/2026-11-02-mcp-spec-rmcp-litellm.md`.

## Global Constraints

- Work only in the worktree `../litegen-mcp` on branch `mcp` (created from `3d-models` with superpowers:using-git-worktrees). Never run cargo in the shared checkout (the main repo folder); other sessions build there.
- Build and test with `CARGO_INCREMENTAL=0` from `litegen-core/`.
- `rmcp = { version = "3.8", default-features = false, features = ["base64", "server", "transport-streamable-http-server"] }`. Never enable rmcp's `local` feature.
- No new field on `AppState`. MCP settings live in `AppConfig.mcp`, read through `state.router.config`.
- Apart from Task 0's three user-approved changes (privilege-checked key edits, AI read routes accepting `generate`, Keys page presets), the existing HTTP API keeps its behavior: every existing test passes unchanged, including `src/api/middleware/auth_tests.rs` and `dashboard/e2e/god-test.spec.ts`.
- No subagents, for implementation or review.
- `src/mcp/` never touches `src/mesh/` (`.claude/rules/mesh-standalone.md`).
- Tool names match `^[a-z][a-z0-9_]{2,40}$` and are unique. Input-schema property names match `^[A-Za-z0-9_.-]{1,64}$`. Every input schema is valid JSON Schema 2020-12 with `"type": "object"` at the root and no root `allOf`/`oneOf`/`anyOf`.
- Every tool has a `title` and all four annotation hints set explicitly (`read_only_hint`, `destructive_hint`, `idempotent_hint`, `open_world_hint`).
- Tool result text is at most 100,000 characters; `structuredContent` is the same value as the text.
- `wait_for_generation`: default timeout 45 s, maximum 230 s, poll every 2 s. Progress heartbeats every 10 s when the client sent a progress token.
- Image previews: longest side ≤ 512 px, JPEG quality 80, at most 2 per call, off when the tool argument `preview` is `false`.
- Session cookies are never read on `/mcp` or `/admin/mcp`.
- Commits: `git commit -m "<type>(mcp): <sentence>" -- <paths>` with an explicit pathspec, never `-a`, never a Claude/Anthropic trailer or mention. Commit only after the user has said commits on branch `mcp` are fine.
- Docs prose: plain, specific, no marketing. Each client page names the date and official sources it was checked against.

## Review Focus

1. **Ids with slashes in path parameters.** Model ids (`fal/flux-pro`) go into `/v1/models/{id}` and `/v1/admin/models/{id}`, which are catch-all routes. `get_model {"id": "<a real catalog id with a slash>"}` must return that model's schema, not 404. Test in Task 6.
2. **A model sends bad arguments** (wrong type, unknown field, missing required field). The caller must get the API's own 4xx message as `isError: true` text, never a JSON-RPC error and never a 500. Test in Task 6.
3. **Oversized results** (`list_models` over the full shipped catalog, a large logs page). The text must stay ≤ 100,000 characters and say what was cut and how to page or filter. Test in Task 6.
4. **Protocol revisions.** A 2025-11-25 client (`initialize`, then `tools/list` with no session id) and a 2026-07-28 client (no `initialize`; `Mcp-Method` header and `_meta` protocol fields) must both work against the stateless server. Test in Task 1 and Task 5.
5. **Proxied deployments.** A request whose `Host` is the host of `public_base_url` passes; a request with a foreign `Origin` gets 403; a request with no `Origin` passes. Test in Task 2.

---

### Task 0: Key scopes: privilege-checked edits, AI-only keys, presets on the Keys page

Do the three parts in this order; part (a) is a security fix and lands first.

**Files:**
- Modify: `litegen-core/src/api/middleware/mod.rs` (add `validate_grant` and `check_any_scope`)
- Modify: `litegen-core/src/api/handlers/mod.rs` (`create_api_key` ~line 2592 uses `validate_grant`; `patch_api_key_handler` ~line 2885 calls it; `create_router` ~line 3849 splits `read_routes`)
- Modify: `litegen-core/src/types/mod.rs` (doc comments on `CreateApiKeyRequest.scopes` and `UpdateApiKeyRequest.scopes`: what `generate`, `read` and `admin` each open)
- Modify: `litegen-core/tests/harness/mod.rs` (add `app_over_with`)
- Modify: `docs/management-api.md` (new "Key scopes" section: scope → routes table, presets ↔ LiteLLM key types)
- Create: `litegen-core/tests/key_scopes_api.rs`
- Create: `dashboard/src/pages/key-scopes.ts`, `dashboard/src/pages/key-scopes.test.ts`
- Modify: `dashboard/src/pages/Keys.tsx`
- Regenerate: `sdks/openapi.json` and the generated SDK types (the scope descriptions change). Tell the user before landing it (shared-tree rule).

**Interfaces:**
- Produces:
  - (a) `pub(crate) fn validate_grant(ctx: &KeyContext, scopes_csv: &str) -> Result<(), Response>`. This is the loop now inline in `create_api_key`, moved verbatim, with the same messages: an unknown token → 400 `unknown scope '{t}'`; a token not in `grantable_scopes(ctx)` → 403 `insufficient privilege to grant scope '{t}'`.
    - `create_api_key` calls it.
    - `patch_api_key_handler` calls it when `req.scopes` is `Some`, after the ownership check and before `update_api_key`.
  - `tests/harness/mod.rs`: `pub async fn app_over_with(db: Arc<SqliteDatabase>, asset_dir: &Path, tweak: impl FnOnce(&mut AppConfig), master_key: Option<&str>, mode: Mode) -> Router`. The existing `app_over` delegates to it with `(None, Mode::SingleTenant)`, so other suites are unchanged. Task 1 and later tasks reuse it.
  - (b) `pub async fn check_any_scope(required: &'static [Scope], request: Request, next: Next) -> Response` returns the same 401/403 bodies as `check_scope`, and passes when `ctx.scopes` holds any of `required`.
    - In `create_router`, `ai_read_routes` holds `/v1/models`, `/v1/models/{*id}`, `/v1/providers`, `/v1/utils/models3d/formats`, `/v1/generations`, `/v1/generations/{id}` (GET and PATCH), `/v1/generations/{id}/assets` and `/v1/generations/{id}/assets/{index}`, behind `check_any_scope(&[Scope::Generate, Scope::Read])`.
    - `read_routes` keeps `/health`, `/v1/stats`, `/v1/logs` and `/v1/logs/{id}/artifact` behind `check_scope(Scope::Read)`.
    - Both keep the CSRF layer.
  - (c) `dashboard/src/pages/key-scopes.ts`:
    - `export type ScopePreset = 'ai' | 'management' | 'ai_management' | 'read_only' | 'custom'`
    - `export const PRESETS: Record<Exclude<ScopePreset, 'custom'>, { label: string; scopes: string; description: string; needsAdmin: boolean }>` with these entries:
      - `ai` → "AI", `generate`, "Generate images, video and 3D, and read models and its own generations. No management access.", false
      - `management` → "Management", `read,admin`, "Manage keys, budgets, end users and settings, and read spend and logs. Cannot generate.", true
      - `ai_management` → "AI + management", `generate,read,admin`, "Everything this key's org allows.", true
      - `read_only` → "Read-only", `read`, "Read models, generations, logs, spend and limits. Changes nothing.", false
    - `export function presetFor(scopes: string): ScopePreset` ignores order, spaces and duplicates; unmatched → `'custom'`.
    - `export function scopeLabel(scopes: string): string` returns the preset label, else the scopes as written.
    - `export function availablePresets(canGrantAdmin: boolean): ScopePreset[]` lists the presets in the order above plus `'custom'`, dropping `needsAdmin` presets when false.
  - `Keys.tsx`:
    - The create and edit forms each get a preset radio group (`data-testid="new-key-preset"`, `"edit-key-preset"`) above the existing scopes text field, which keeps its test ids `new-key-scopes` and `edit-scopes`.
    - Choosing a preset writes its scopes into the field; typing in the field selects Custom.
    - The create form defaults to AI.
    - `canGrantAdmin` = the user's role permissions include `key:write:any` (reuse `permissionsFor` from `components/RequirePermission.tsx`).
    - The key list shows `scopeLabel(k.scopes)`, with the raw scopes in a `title` tooltip.

- [ ] **Step 1: Write the failing tests** in `tests/key_scopes_api.rs`, over `harness::app_over_with` with a master key in single-tenant mode, using a Member session (sign up and log in the way `tests/multitenant_api.rs` does, adding the member through the owner):
  - (a) `member_cannot_edit_own_key_to_admin`: the Member creates a `generate,read` key (201), then `PATCH /v1/keys/{id}` with `{"scopes": "admin"}` → 403 `insufficient privilege to grant scope 'admin'`, and `GET /v1/keys/{id}` still shows `generate,read`.
  - (a) `edit_rejects_unknown_scope`: an owner PATCHes `{"scopes": "generate,admn"}` → 400 `unknown scope 'admn'`.
  - (a) `owner_can_edit_to_admin`: an owner PATCHes their key to `admin` → 200.
  - (b) `generate_only_key_reads_ai_routes`: a `generate` key → 200 on `/v1/models`, `/v1/providers`, `/v1/generations` and `/v1/utils/models3d/formats`.
  - (b) `generate_only_key_has_no_management_reads`: the same key → 403 `forbidden_scope` on `/v1/logs` and `/v1/stats`, and 403 on `/v1/spend/report` and `/v1/limits`.
  - (b) `read_only_key_unchanged`: a `read` key → 200 on all of the above reads, 403 on `POST /v1/images/generations`.
  - (c) vitest `key-scopes.test.ts`:
    - `presetFor('read, generate ,admin') === 'ai_management'`;
    - `presetFor('generate,read') === 'custom'`;
    - `scopeLabel('read,admin') === 'Management'`;
    - `availablePresets(false)` excludes `management` and `ai_management`.
- [ ] **Step 2: Run** `CARGO_INCREMENTAL=0 cargo test --test key_scopes_api` and `npm --prefix dashboard test -- key-scopes` → FAIL.
- [ ] **Step 3: Implement (a)**, run its three tests → PASS, and commit `fix(keys): privilege-check scope changes on key edits`.
- [ ] **Step 4: Implement (b)** and the `types/mod.rs` doc comments. Run `cargo test --test key_scopes_api` and the whole crate `CARGO_INCREMENTAL=0 cargo test` → PASS. Regenerate the SDK artifacts with `sdks/scripts/regen-all.sh` (in the worktree) and run the TS and Python SDK tests → PASS. Commit `feat(keys): AI read routes accept the generate scope, so generate alone is an AI-only key`.
- [ ] **Step 5: Implement (c)** and the `docs/management-api.md` section. Run `npm --prefix dashboard test`, `npm --prefix dashboard run lint`, `npm --prefix dashboard run build`, and the e2e run of `god-test.spec.ts` that creates keys (`npm --prefix dashboard run test:e2e -- god-test`) → PASS. Commit `feat(dashboard): LiteLLM-style scope presets on the Keys page`.

### Task 1: Config, module skeleton and the two endpoints

**Files:**
- Modify: `litegen-core/Cargo.toml` (add rmcp; add `features = ["util"]` to the normal `tower` dependency; add dev-dependency `jsonschema`, latest release)
- Modify: `litegen-core/src/lib.rs` (`pub mod mcp;`)
- Modify: `litegen-core/src/config/mod.rs` (`McpConfig`, `AppConfig.mcp`, its `Default`, and the redacting serializer near line 689)
- Modify: `litegen-core/src/api/handlers/mod.rs` (`create_router`, around line 3988)
- Modify: `litegen.example.yaml` (commented `mcp:` section, values from the spec)
- Create: `litegen-core/src/mcp/mod.rs`, `litegen-core/src/mcp/server.rs`, `litegen-core/src/mcp/instructions/ai.md`, `litegen-core/src/mcp/instructions/admin.md`
- Create: `litegen-core/tests/mcp_support/mod.rs`, `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Consumes: `harness::app_over_with` (Task 0).
- Produces:
  - `pub struct McpConfig { pub enabled: bool, pub admin_enabled: bool, pub allowed_hosts: Vec<String>, pub allowed_origins: Vec<String> }` (serde defaults `true`, `true`, `[]`, `[]`).
  - `pub enum Surface { Ai, Admin }` with `fn path(self) -> &'static str` (`"/mcp"`, `"/admin/mcp"`), `fn toolsets(self) -> &'static [&'static str]`, `fn instructions(self) -> &'static str` (`include_str!` of the two `.md` files).
  - `pub struct McpShared { pub state: Arc<AppState>, pub api: axum::Router, pub registry: registry::Registry, pub http: reqwest::Client }` (registry is empty until Task 5).
  - `pub fn router(state: Arc<AppState>, api: axum::Router) -> axum::Router` mounts `route_service(Surface::Ai.path(), …)` and, when `admin_enabled`, `route_service(Surface::Admin.path(), …)`. Use `route_service`, not `nest_service`: `/mcp` must not swallow `/admin/mcp`.
  - `pub struct LiteGenMcp { pub surface: Surface, pub shared: Arc<McpShared> }` implementing `rmcp::ServerHandler::get_info` → `ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(Implementation { name: "litegen", title: "LiteGen" (AI) / "LiteGen admin", version: env!("CARGO_PKG_VERSION"), .. }).with_instructions(surface.instructions())`.
  - The rmcp config: `NeverSessionManager`, `.with_legacy_session_mode(false)`, `.with_json_response(true)`, `.with_max_request_body_bytes(16 * 1024 * 1024)`.
  - Test helpers in `tests/mcp_support/mod.rs`:
    - `pub async fn app(tweak: impl FnOnce(&mut AppConfig)) -> (Router, Arc<SqliteDatabase>, TempDir)`: dev mode.
    - `pub async fn secured_app(mode: Mode) -> (Router, Arc<SqliteDatabase>, TempDir)`: master key `"test-master"`.
    - `pub struct Mcp<'a> { app: &'a Router, path: &'a str, bearer: Option<String>, protocol: &'static str }` with `async fn rpc(&self, method: &str, params: Value) -> Rpc`.
    - `pub struct Rpc { pub status: StatusCode, pub result: Option<Value>, pub error: Option<Value>, pub notifications: Vec<Value> }`.
    - `rpc` posts with `Host: localhost`, `Content-Type: application/json`, `Accept: application/json, text/event-stream` and `MCP-Protocol-Version`. For `2026-07-28` it also sends `Mcp-Method` (and `Mcp-Name` for `tools/call`) and puts `io.modelcontextprotocol/protocolVersion`, `.../clientCapabilities` and `.../clientInfo` into `params._meta`. It parses a JSON body or an SSE body (`data:` lines).
    - `pub const LEGACY: &str = "2025-11-25"; pub const CURRENT: &str = "2026-07-28";`

- [ ] **Step 1: Write the failing integration tests** in `tests/mcp_api.rs`:
  - `initialize_answers_on_both_endpoints_legacy`: `Mcp{protocol: LEGACY}.rpc("initialize", {"protocolVersion": LEGACY, "capabilities": {}, "clientInfo": {"name":"t","version":"0"}})` on `/mcp` → `result.serverInfo.name == "litegen"`, `result.instructions` starts with `"LiteGen generates images, video and 3D models"`; on `/admin/mcp` → instructions start with `"LiteGen management"`.
  - `discover_answers_on_current_protocol`: `rpc("server/discover", {})` with `CURRENT` → status 200, `result.instructions` as above, `result.supportedVersions` contains `"2026-07-28"`.
  - `get_is_405`: `GET /mcp` → 405.
  - `disabled_endpoints_are_404`: `mcp.enabled = false` → `POST /mcp` 404; `mcp.admin_enabled = false` → `/admin/mcp` 404 while `/mcp` still answers.
- [ ] **Step 2: Run** `CARGO_INCREMENTAL=0 cargo test --test mcp_api` → FAIL (module `mcp` missing).
- [ ] **Step 3: Implement** the files above. Write the two instruction files: their first 512 characters must make sense on their own, and each whole file stays under 4,096 characters. Use this exact opening for `ai.md`: "LiteGen generates images, video and 3D models through one API. To generate: 1) list_models (media_type image, video or model3d), 2) get_model for that model's parameters, 3) optionally estimate_image_cost / estimate_video_cost / estimate_3d_model_cost, 4) generate_image (returns images) or generate_video / generate_3d_model (return a job id), 5) wait_for_generation until it completes. Every call spends the budget of the key or account you connected with." For `admin.md`: "LiteGen management: API keys, budgets and limits, end users, spend, request logs, audit log, organizations, apps, members and gateway configuration. Tools mirror the REST management API one to one and run with your key's scopes (read for reads, admin for changes) or your account role; tools you cannot use are not listed. Destructive tools (revoke, delete, block, reset, transfer) act immediately." In `create_router`, clone `state` before `.with_state(state)`, build the API router as now, then return `api.clone().merge(crate::mcp::router(state, api))` when `mcp.enabled`, else `api`.
- [ ] **Step 4: Run** `CARGO_INCREMENTAL=0 cargo test --test mcp_api` → PASS. If rmcp refuses a legacy `tools/list` without a prior `initialize` on the same connection, note it for Task 5's protocol test.
- [ ] **Step 5: Commit** `feat(mcp): serve /mcp and /admin/mcp with rmcp, stateless`, with the files above as pathspec.

### Task 2: Who is calling: API keys, master key, dev mode; 401, Host and Origin

**Files:**
- Modify: `litegen-core/src/api/middleware/mod.rs` (extract from `auth_middleware`, lines 234-440 and `handle_db_key` 488-645)
- Modify: `litegen-core/src/api/cors.rs` (extract the origin list from `layer`, lines 44-47)
- Create: `litegen-core/src/mcp/auth.rs`
- Modify: `litegen-core/src/mcp/mod.rs` (layer `mcp_auth` over both routes; Host/Origin lists into the rmcp config)
- Test: `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Produces:
  - `pub(crate) async fn resolve_api_key(state: &AppState, token: &str) -> Result<KeyContext, Response>` handles the master key, the single-tenant dev bypass with a token, and DB keys (expiry, quota pre-check, scopes). It is exactly the existing logic minus the RPM step. `auth_middleware` now calls it and keeps the RPM step and headers for DB keys.
  - `pub(crate) fn dev_bypass_context(state: &AppState) -> Option<KeyContext>` returns the no-credential single-tenant context, or `None`.
  - `pub fn configured_origins(config: &CorsConfig, origins_override: Option<&str>) -> Vec<String>`, used by `cors::layer` too.
  - `pub enum CallerKind { ApiKey, MasterKey, Dev }` (plan 2 adds `OAuth`).
  - `pub struct Caller { pub ctx: KeyContext, pub kind: CallerKind, pub authorization: Option<HeaderValue>, pub surface: Surface, pub toolsets: Option<Vec<String>> }`.
  - `pub async fn mcp_auth(State(shared): State<Arc<McpShared>>, req: Request, next: Next) -> Response`, in this order:
    1. An `Origin` that is present and not allowed → 403.
    2. `toolsets` query parsing. An unknown name → 400 `{"error":{"code":"unknown_toolset","message":"unknown toolset 'x'; valid toolsets: …"}}`.
    3. Bearer → `resolve_api_key`. A 401 from it becomes the challenge. Any other error status is returned as is.
    4. No Bearer → `dev_bypass_context`, else the challenge.
    5. Insert `Caller` into the request extensions.
  - `pub fn challenge(surface: Surface, body: Option<Response>) -> Response`: 401 with `WWW-Authenticate: Bearer realm="litegen"` and the API's JSON error body. Plan 2 adds `resource_metadata` and `scope`.
  - Allowed hosts: `localhost`, `127.0.0.1`, `::1`, the host (with port, if any) of `public_base_url`, and `mcp.allowed_hosts`. `["*"]` → `disable_allowed_hosts()`.
  - Allowed origins: `configured_origins(…)` + the origin of `public_base_url` + `mcp.allowed_origins`. `"*"` anywhere → any origin.

- [ ] **Step 1: Write failing tests** in `tests/mcp_api.rs`:
  - `hosted_without_credentials_is_401_with_challenge`: `secured_app(Hosted)`, no bearer → 401 and a `www-authenticate` header starting with `Bearer`.
  - `invalid_key_is_401`: bearer `"sk_live_nope"` → 401.
  - `valid_key_initializes`: create a key with scopes `"generate,read"` via `POST /v1/keys` (master key) → `initialize` 200.
  - `session_cookie_is_ignored`: sign up and log in through `/v1/auth/signup` and `/v1/auth/login` (hosted mode allows signup) to get a real `litegen_session` cookie. Send it with no bearer → 401.
  - `foreign_origin_is_403`: `Origin: https://evil.example` → 403.
  - `public_host_passes`: config `server.public_base_url = "https://app.example.com/api"`, `Host: app.example.com` → 200.
  - `unknown_toolset_is_400`: `/mcp?toolsets=images,nope` → 400 with `unknown_toolset`.
- [ ] **Step 2: Run** `cargo test --test mcp_api` → the new tests FAIL.
- [ ] **Step 3: Implement** the interfaces above.
- [ ] **Step 4: Run** `cargo test --test mcp_api` and `cargo test --lib api::middleware` → all PASS. The existing auth tests are unchanged.
- [ ] **Step 5: Commit** `feat(mcp): authenticate MCP calls with the API's own key resolution`.

### Task 3: The tool catalog and its gates

**Files:**
- Create: `litegen-core/src/mcp/catalog.rs`, `litegen-core/src/mcp/gate.rs`, `litegen-core/src/mcp/tests.rs`
- Modify: `litegen-core/src/api/handlers/management/mod.rs` (none expected: `require_gateway_admin` is already `pub(crate)`)

**Interfaces:**
- Produces:
  - `pub enum Http { Get, Post, Put, Patch, Delete }`
  - `pub enum Surfaces { Ai, Admin, Both }`
  - `pub enum Gate { Scope(Scope), AnyScope(&'static [Scope]), OrgRead(Permission), OrgWrite(Permission), Member(Permission), Perm(Permission), Global(Permission), GatewayAdmin, User, Any }`
  - `pub struct ToolDef { pub name: &'static str, pub method: Http, pub path: &'static str, pub surfaces: Surfaces, pub toolset: &'static str, pub gate: Gate, pub title: &'static str, pub description: Option<&'static str>, pub catch_all: Option<&'static str> }`. `path` is the OpenAPI form (`/v1/models/{id}`). `catch_all` names the parameter whose `/` must survive: `id` for `get_model`, `get_gateway_model`, `replace_gateway_model`, `update_gateway_model`, `delete_gateway_model`.
  - `pub struct Excluded { pub method: Http, pub path: &'static str, pub reason: &'static str }`
  - `pub static TOOLS: &[ToolDef]` (the 112 rows of Appendix A) and `pub static EXCLUDED: &[Excluded]` (the 20 operations listed in the spec's "Excluded operations", with these reasons: "browser sign-in flow; MCP clients connect with an API key or OAuth", "invitation link meant for a person", "browser-session settings", "infrastructure probe").
  - `pub fn visible(def: &ToolDef, caller: &Caller, state: &AppState) -> bool` mirrors each route's own check:
    - `Scope(s)`: `ctx.scopes ∋ s` (`check_scope`). `AnyScope(ss)`: any of them.
    - `OrgRead(p)`: platform admin; or a user whose `ctx.permissions ∋ p` and `ctx.scopes ∋ Read`; or a key with `Read` or `Admin` (`management::require_org`).
    - `OrgWrite(p)`: platform admin; or a user with `p` and `Admin` scope; or a key with `Admin`.
    - `Member(p)`: platform admin, or a user with `p` and the method's scope (`Read` for GET, else `Admin`) (`orgs::require_member_perm`).
    - `Perm(p)`: `ctx.permissions ∋ p` plus the method's scope (`users::invite_user`).
    - `Global(p)`: hosted → platform admin; single-tenant → `ctx.permissions ∋ p` (`users::authorize_global_user_admin`).
    - `GatewayAdmin`: `management::require_gateway_admin(state, &ctx).is_ok()`.
    - `User`: `ctx.user.is_some()`. `Any`: true.
    - Then: the tool's surface must include the caller's surface, and its toolset must be in `caller.toolsets` when that is set. On the AI surface, tools outside the `catalog` toolset are hidden when `ctx.org_id` is `None` (the hosted master key): their handlers refuse without a tenant (`forbidden_no_org`). Check each `catalog` handler works without an org, and hide it too if it does not.
  - Titles: sentence case of the name with "API", "3D" and "MCP" capitalized ("Generate 3D model", "Revoke API key").
  - Description overrides: Appendix B. All other tools take their OpenAPI summary and description.

- [ ] **Step 1: Write failing unit tests** in `src/mcp/tests.rs`:
  - `every_openapi_operation_is_a_tool_or_excluded`: from `serde_json::to_value(ApiDoc::openapi())`, every (method, path) is in exactly one of `TOOLS`/`EXCLUDED`, and neither list names an operation the document lacks.
  - `tool_names_are_valid_and_unique`: regex `^[a-z][a-z0-9_]{2,40}$`, no duplicates.
  - `toolsets_are_known`: each `toolset` is in its surface's list (`Both` → in both lists).
  - `gates_match_principals`: build contexts for a `generate`-only key, a `generate,read` key, a `read`-only key, an `admin`-only key, a `generate,read,admin` key, the hosted master key, the single-tenant master key, and dev mode. Assert:
    - the `generate`-only key sees `generate_image` and `list_models` (Task 0 opened the AI reads to it) but not `list_request_logs` or `get_spend_report`;
    - the `read`-only key sees `list_models` and `get_spend_report` but not `generate_image`;
    - the `admin`-only key sees `set_org_limits` and `get_org_limits` (admin implies read in `require_org`) but not `list_request_logs` (`check_scope(Read)` is exact);
    - no key sees `list_orgs` or `create_org` (they need a signed-in user);
    - the hosted master key sees `list_gateway_models` and `list_users`, but on the AI surface only `catalog` tools;
    - the single-tenant master key does not see `list_users` (its context has no permissions, so `authorize_global_user_admin` refuses it).
- [ ] **Step 2: Run** `CARGO_INCREMENTAL=0 cargo test --lib mcp::tests` → FAIL.
- [ ] **Step 3: Implement** `catalog.rs` from Appendix A and `gate.rs` as specified.
- [ ] **Step 4: Run** `cargo test --lib mcp::tests` → PASS.
- [ ] **Step 5: Commit** `feat(mcp): catalog every API operation with the gate its route enforces`.

### Task 4: Input schemas from the OpenAPI document

**Files:**
- Create: `litegen-core/src/mcp/schema.rs`
- Test: `litegen-core/src/mcp/tests.rs`

**Interfaces:**
- Consumes: `ToolDef` (Task 3).
- Produces:
  - `pub struct ArgPlan { pub path: Vec<String>, pub query: Vec<String>, pub body: BodyPlan, pub headers: Vec<(String, &'static str)>, pub binary_in: bool, pub binary_out: bool }`
  - `pub enum BodyPlan { None, Flattened(Vec<String>), Nested }`
  - `pub fn build(doc: &serde_json::Value, def: &ToolDef) -> Result<(serde_json::Map<String, Value>, ArgPlan), String>`
  - Rules:
    - Inline every `$ref` (a cycle becomes `{"type":"object"}`).
    - Merge `allOf` members into one object.
    - `type: [T, "null"]` → `T`; `oneOf`/`anyOf` of `[X, {"type":"null"}]` → `X`.
    - Drop `format: binary`, `contentMediaType` and `$schema`.
    - Path, query and header parameters become properties. Header `Idempotency-Key` becomes `idempotency_key`.
    - A JSON object body is flattened into the root. If a body property collides with a parameter name, or the body is not an object, the whole body goes under `body` instead (`BodyPlan::Nested`).
    - An `application/octet-stream` request body becomes two properties, `source_url` (string, uri) and `data_base64` (string), described as "exactly one of these", with `binary_in = true`.
    - A non-JSON success response sets `binary_out`.
    - The four billed tools (`generate_image`, `segment_image`, `generate_video`, `generate_3d_model`) also get `end_user` (string → `x-litegen-end-user`) and `tags` (array of string → `x-litegen-tags`).
    - `generate_image` and `get_generation_asset` get `preview` (boolean, default true).
    - A catch-all parameter's description adds "may contain '/' (send it unescaped)".

- [ ] **Step 1: Write failing tests**:
  - `schemas_have_no_refs_and_object_roots`: for all 112 tools, the serialized schema has no `"$ref"`, the root has `"type": "object"`, and there is no root `allOf`/`oneOf`/`anyOf`.
  - `schemas_validate_against_2020_12`: each schema validates against the Draft 2020-12 meta-schema (`jsonschema` dev-dependency).
  - `property_names_are_portable`: every top-level property matches `^[A-Za-z0-9_.-]{1,64}$`.
  - `generate_image_flattens_body`: properties include `model`, `prompt`, `n`, `idempotency_key`, `end_user`, `tags`, `preview`; `required` includes `model` and `prompt` exactly as the OpenAPI schema requires.
  - `convert_takes_url_or_base64`: `convert_3d_model` has `source_url` and `data_base64`, and `binary_in` is set.
  - `ai_surface_schema_budget`: the summed JSON length of all AI-surface schemas is < 90,000 characters (measured today ≈ 51,000).
- [ ] **Step 2: Run** `cargo test --lib mcp::tests` → FAIL.
- [ ] **Step 3: Implement** `schema.rs`.
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(mcp): derive tool input schemas from the OpenAPI document`.

### Task 5: Registry and `tools/list`

**Files:**
- Create: `litegen-core/src/mcp/registry.rs`
- Modify: `litegen-core/src/mcp/server.rs`, `litegen-core/src/mcp/mod.rs`
- Test: `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Consumes: `TOOLS`, `visible` (Task 3), `schema::build` (Task 4), `Caller` (Task 2).
- Produces:
  - `pub enum SpecKind { Http(ArgPlan), Wait }`
  - `pub struct ToolSpec { pub def: &'static ToolDef, pub kind: SpecKind, pub tool: rmcp::model::Tool }`
  - `pub struct Registry { specs: Vec<ToolSpec> }` with:
    - `pub fn build(doc: &Value) -> Result<Registry, String>`: fails startup with the offending tool's name.
    - `pub fn get(&self, name: &str) -> Option<&ToolSpec>`.
    - `pub fn visible(&self, caller: &Caller, state: &AppState) -> Vec<&ToolSpec>`: in catalog order, deterministic.
  - Annotations (`ToolAnnotations`):
    - `read_only`: GET, the three `estimate_*` tools, and `wait_for_generation`.
    - `destructive`: DELETE and any tool whose name starts with `revoke_`, `delete_`, `block_`, `remove_`, `reset_`, `rotate_`, `transfer_` or `clear_`.
    - `idempotent`: GET, PUT and DELETE.
    - `open_world`: the generation, `segment_image` and `wait_for_generation` tools.
  - `list_tools` reads `Caller` from `ctx.extensions.get::<http::request::Parts>()` → `parts.extensions.get::<Caller>()` and returns `ListToolsResult` with all visible tools, `ttl_ms = 300_000` and a private cache scope (`tools/list` varies by credential).
  - Description cleanup for OpenAPI-derived text: drop lines starting with `Live:`, code fences and internal plan references such as `(L14)` and `(3D doc §4)`; at most 1,200 characters.
  - `wait_for_generation` is registered here as `SpecKind::Wait` with a hand-written schema: `id` (string, required) and `timeout_seconds` (integer, 1–230, default 45). Its gate is `AnyScope(&[Generate, Read])`; toolset `generations`; AI surface. Its behavior is Task 8.

- [ ] **Step 1: Write failing tests**:
  - `ai_key_is_exactly_ai`: a `generate` key (the Keys page's "AI" preset) sees 21 tools on `/mcp` (all AI tools except `get_current_user`, which needs a signed-in user), and on `/admin/mcp` only `get_my_limits`.
  - `default_key_lists_ai_and_admin_reads`: a `generate,read` key sees 21 tools on `/mcp` (all AI tools except `get_current_user`, which needs a signed-in user). On `/admin/mcp` it sees `get_spend_report` but not `create_api_key`.
  - `admin_key_lists_key_tools`: a `generate,read,admin` key sees `create_api_key`, `revoke_api_key`, `set_key_limits` on `/admin/mcp`.
  - `toolsets_filter`: `/mcp?toolsets=images` → exactly `estimate_image_cost`, `generate_image`, `segment_image`.
  - `annotations_are_explicit`: every listed tool has a title and all four hints present; `revoke_api_key` is destructive; `estimate_image_cost` is read-only.
  - `legacy_and_current_protocols_list_tools`: the same key gets the same names via `LEGACY` (`initialize`, then `tools/list` as separate POSTs with no session id) and via `CURRENT`.
- [ ] **Step 2: Run** `cargo test --test mcp_api` → FAIL.
- [ ] **Step 3: Implement**; build the `Registry` once in `mcp::router` (startup failure is a panic with the message).
- [ ] **Step 4: Run** `cargo test --test mcp_api` and `cargo test --lib mcp::tests` → PASS.
- [ ] **Step 5: Commit** `feat(mcp): list each caller the tools its credential can call`.

### Task 6: `tools/call` through the API router, JSON results

**Files:**
- Create: `litegen-core/src/mcp/dispatch.rs`, `litegen-core/src/mcp/content.rs`
- Modify: `litegen-core/src/mcp/server.rs`
- Test: `litegen-core/src/mcp/tests.rs`, `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Consumes: `Registry`, `ToolSpec`, `ArgPlan`, `Caller`.
- Produces:
  - `pub struct HttpReply { pub status: StatusCode, pub headers: HeaderMap, pub body: Bytes }`
  - `pub fn build_request(spec: &ToolSpec, plan: &ArgPlan, caller: &Caller, args: &JsonObject) -> Result<http::Request<axum::body::Body>, String>`:
    - Path parameters are percent-encoded, except a `catch_all` parameter, which keeps its `/`.
    - Query scalars use `to_string`; arrays are comma-joined.
    - Body: JSON of the flattened or nested properties.
    - Headers: `Authorization` from `caller.authorization`; `Idempotency-Key`, `x-litegen-end-user`, and `x-litegen-tags` = `mcp` plus the caller's tags (comma-joined).
    - Unknown arguments are forwarded inside the body for `Flattened` tools (the API decides), and are an error otherwise.
    - A missing required path parameter is an `Err` whose message names it.
  - `pub async fn send(api: &axum::Router, req: Request<Body>) -> HttpReply` uses `api.clone().oneshot(req)` and reads the body up to 64 MiB.
  - `pub fn json_result(spec: &ToolSpec, reply: &HttpReply) -> CallToolResult`:
    - 2xx: `structured_content` is the parsed body (arrays wrapped as `{"items": [...]}`), and the text block is the same value as compact JSON.
    - Non-2xx: `is_error: true`, text `"HTTP {status} {code}: {message}"` from the API's `{"error":{…}}` body, and `structured_content` `{"status": n, "error": {…}}`.
    - An `x-litegen-dropped-params` header adds the text block `"Note: LiteGen ignored these parameters for this model (strict: false): {list}"`.
    - An `idempotent-replayed: true` header adds `"Note: replayed from an earlier call with the same idempotency_key; nothing new was billed."`
  - `pub fn fit(value: Value, max_chars: usize) -> Value`: while the compact JSON is over `max_chars` (100,000), halve the largest top-level array (or `data`/`items`), then add `"_truncated": {"shown": n, "total": m, "hint": "Fewer results: use the tool's page/per_page or filter arguments."}`. For `list_models`, first project each entry to `id, name, provider, media_type, status, is_available, tags` and add `"_compact": "call get_model for a model's full schema"`.
  - `call_tool`:
    1. Look up the spec; it must be visible to the caller, else `Ok(CallToolResult::error("Unknown tool: {name}"))`.
    2. `build_request`. Its `Err` → `isError` with the message.
    3. `send`, then `json_result`, then `fit`.
    4. Record `metrics::counter!("litegen_mcp_tool_calls_total", "tool" => name, "surface" => …, "outcome" => "ok" | "error")`, with a `tracing` span carrying tool, surface, caller kind and status.

- [ ] **Step 1: Write failing tests**:
  - Unit: `build_request_keeps_slash_in_catch_all`: `get_model` with `id = "fal/flux-pro"` → URI path `/v1/models/fal/flux-pro`.
  - Unit: `build_request_encodes_regular_params`: an `end_user_id` of `a b/c` → `/v1/end-users/a%20b%2Fc`.
  - Unit: `tags_always_include_mcp`: `tags: ["x"]` → header `mcp,x`.
  - Integration:
    - `generate_image_with_mock_returns_data`: a `generate,read` key; `tools/call generate_image {"model": <the mock image model the harness registers>, "prompt": "a red cube", "preview": false}` → `isError` absent or false, and `structuredContent.data[0]` has `url` or `b64_json`.
    - `get_model_with_slash_id`: `get_model {"id": <first catalog id containing '/'>}` → `structuredContent.id` equals it.
    - `bad_arguments_return_api_error`: `generate_image {"model": "mock/…", "prompt": 5}` → `isError: true`, text starting `HTTP 4`, and no JSON-RPC `error`.
    - `unknown_model_is_api_error`: `{"model": "nope/nope", "prompt": "x"}` → `isError: true`, text contains the API's code.
    - `hidden_tool_call_is_refused`: a `generate` key calls `list_request_logs` on `/admin/mcp` → `isError: true`, text `Unknown tool: list_request_logs`.
    - `budget_is_enforced_through_mcp`: set the key's limits with `max_budget_usd: 0.0000001` through `set_key_limits` (admin key), then `generate_image` with that key → `isError` and text containing `budget_exceeded`.
    - `list_models_fits`: `list_models {}` text length ≤ 100,000.
    - `generation_is_tagged_mcp`: after a generation, `GET /v1/spend/report?group_by=tag` (HTTP, admin key) has a row for tag `mcp`.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --test mcp_api` and `cargo test --lib mcp::tests` → PASS.
- [ ] **Step 5: Commit** `feat(mcp): run tool calls through the API router and shape their results`.

### Task 7: Images, assets and mesh files

**Files:**
- Modify: `litegen-core/src/mcp/content.rs`, `litegen-core/src/mcp/dispatch.rs`, `litegen-core/src/mcp/server.rs`
- Test: `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Produces:
  - `pub enum ImageSource { Url(String), Base64(String) }`.
  - `pub async fn previews(shared: &McpShared, caller: &Caller, sources: Vec<ImageSource>) -> Vec<ContentBlock>`. Each is an image block (JPEG, ≤ 512 px longest side, quality 80; at most 2; resize in `spawn_blocking`).
    - An `ImageSource::Url` under `public_base_url` is fetched by dispatching a GET through `shared.api`, so it carries the caller's authorization when it has one.
    - Another `http(s)` URL is fetched with `shared.http` (10 s timeout, 16 MiB cap).
    - `ImageSource::Base64` is decoded.
    - Any other scheme, or any failure, yields no preview and no error.
  - `pub fn links(tool: &str, value: &Value) -> Vec<ContentBlock>` returns `resource_link`s:
    - `generate_image`: `data[].url`, mime `image/*`.
    - `get_video_status` / `wait_for_generation`: video `video_url`.
    - `get_3d_model_status` / `wait_for_generation`: `assets[].url`, mime from the format.
    - `list_generation_assets`: `assets[].url`.
  - In text and structured copies, any `b64_json` string longer than 1,024 characters is replaced with `"<{n} bytes of base64 omitted; fetch it with get_generation_asset(id, index)>"`.
  - `convert_3d_model`:
    - Input: `data_base64` decoded, or `source_url` fetched through the materializer's existing safe fetch (find the function the materializer uses for caller-supplied reference URLs and reuse it; do not write a new fetcher), with a 16 MiB cap.
    - Output: an embedded resource (`ResourceContents::blob`, uri `litegen://convert/{uuid}.{ext}`, mime from the format) when ≤ 8 MiB. Larger → `isError` with "The converted file is {n} MiB, over MCP's 8 MiB limit; call POST {public_base_url}/v1/utils/models3d/convert directly."
  - `get_generation_asset`:
    - An image ≤ 4 MiB → preview image plus metadata text.
    - Any other type ≤ 8 MiB → an embedded blob.
    - Larger → a `resource_link` to the asset's `url` from `list_generation_assets`.

- [ ] **Step 1: Write failing tests**:
  - `generate_image_includes_preview_and_link`: the mock image model with `preview` default → `content` has one or two `image` blocks (mime `image/jpeg`, decoded width and height ≤ 512) and a `resource_link` when the API returned a URL.
  - `preview_false_has_no_image`: same with `preview: false` → no image blocks.
  - `convert_round_trip`: `convert_3d_model` with `data_base64` of `tests/fixtures/<a small .obj or .glb fixture already in the repo>` and the target format the endpoint lists → an embedded resource whose decoded bytes start with the target format's magic (`glTF` for glb).
  - `three_d_status_links_assets`: run `mock` 3D to completion (`harness::run_to_completion`), then `get_3d_model_status` → a `resource_link` per asset.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(mcp): return image previews, asset links and converted meshes`.

### Task 8: `wait_for_generation`, progress and heartbeats

**Files:**
- Create: `litegen-core/src/mcp/wait.rs`
- Modify: `litegen-core/src/mcp/server.rs`
- Test: `litegen-core/src/mcp/tests.rs`, `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Produces:
  - `pub fn status_path(id: &str) -> String`: `litegen-vid-…` → `/v1/videos/{id}`, `litegen-3d-…` → `/v1/models3d/{id}`, otherwise `/v1/generations/{id}`.
  - `pub async fn run(shared: &McpShared, caller: &Caller, args: &JsonObject, progress: Option<ProgressSink>) -> CallToolResult`:
    - Polls `status_path` through `dispatch::send` every 2 s, until `status` is `completed`, `failed` or `cancelled`, or the timeout passes.
    - A terminal result is shaped as the status tool's result (links included).
    - At the timeout: the latest state plus the text "Still {status} ({progress}%). Call wait_for_generation again with the same id."
    - A 403/404 from the status route is returned as is (`isError`).
  - `pub struct ProgressSink` wraps the request's progress token and `Peer<RoleServer>`. `async fn report(&self, progress: f64, message: String)` keeps `progress` strictly increasing (use the job's percentage, or elapsed seconds when that does not move).
  - In `call_tool`: when the client sent a progress token, every tool call runs under a 10 s heartbeat that sends `"Working… {elapsed}s"`. `wait_for_generation` sends the job's percentage instead.

- [ ] **Step 1: Write failing tests**:
  - Unit: `status_paths`: three ids → three routes.
  - Integration:
    - `wait_completes_video`: `generate_video` with the mock video model, then `wait_for_generation {"id": …}` → `structuredContent.status == "completed"` and a `resource_link`.
    - `wait_times_out_politely`: `generate_3d_model {"model": "mock/slow-3d", …}`, then `wait_for_generation {"id": …, "timeout_seconds": 1}` → not `isError`, text contains "Call wait_for_generation again".
    - `wait_sends_progress`: the same with `_meta.progressToken = "p1"` and `timeout_seconds: 3` → `Rpc.notifications` contains at least one `notifications/progress` with `progressToken == "p1"`.
    - `wait_respects_tenant`: a key from a second org (create the org and key the way `tests/multitenant_api.rs` does) waiting on the first org's id → `isError` with the API's 404.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** → PASS.
- [ ] **Step 5: Commit** `feat(mcp): wait_for_generation with progress, heartbeats on long calls`.

### Task 9: Protocol edges and observability

**Files:**
- Modify: `litegen-core/src/mcp/mod.rs`, `litegen-core/src/mcp/server.rs`
- Test: `litegen-core/tests/mcp_api.rs`

**Interfaces:**
- Produces:
  - A response layer on both routes that sets `X-Accel-Buffering: no` and `Cache-Control: no-cache` on `text/event-stream` responses.
  - `tracing` span `mcp.request` (method, surface, caller kind).
  - Counter `litegen_mcp_requests_total{surface, method}`.

- [ ] **Step 1: Write failing tests**:
  - `delete_is_405`.
  - `unsupported_protocol_version_is_400`: `MCP-Protocol-Version: 1999-01-01` → 400.
  - `json_only_accept_is_406`: `Accept: application/json` alone → 406 (the transport requires both). Record this in `docs/mcp/troubleshooting.md` in Task 10.
  - `sse_has_no_buffering_header`: a call that emits progress answers `text/event-stream` with `x-accel-buffering: no`.
  - `big_body_is_413`: a 17 MiB POST → 413.
  - `metrics_count_calls`: after one `tools/call`, `GET /metrics` contains `litegen_mcp_tool_calls_total`.
- [ ] **Step 2: Run** → FAIL where behavior is ours (the 405/406/413 checks may already pass from rmcp; keep them as regression tests).
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the whole crate: `CARGO_INCREMENTAL=0 cargo test` and `cargo clippy --all-targets -- -D warnings` → PASS.
- [ ] **Step 5: Commit** `feat(mcp): anti-buffering headers, request metrics and protocol edge tests`.

### Task 10: Generated tool reference and the core docs

**Files:**
- Create: `litegen-core/examples/mcp_tools_doc.rs`, `docs/mcp/tools.md` (generated), `docs/mcp/README.md`, `docs/mcp/permissions.md`, `docs/mcp/self-hosting.md`, `docs/mcp/troubleshooting.md`
- Modify: `README.md`, `sdks/README.md`, `docs/management-api.md` (one link each to `docs/mcp/README.md`), `package.json` (no change)
- Test: `litegen-core/src/mcp/tests.rs`

**Interfaces:**
- Produces:
  - `pub fn render_reference(registry: &Registry) -> String` (in `mcp/registry.rs`) writes per surface, per toolset, one row per tool: name, title, `METHOD /path`, gate in words ("needs the read scope", "org admin role or admin scope", "platform admin"), read-only/destructive. Then one section per tool listing its parameters (name, type, required, first sentence of the description).
  - `cargo run --example mcp_tools_doc > ../docs/mcp/tools.md` produces the file.

- [ ] **Step 1: Write the failing test** `tools_reference_is_current`: `render_reference(&Registry::build(&doc)?) == include_str!("../../../docs/mcp/tools.md")`. On mismatch, the message says to run the example.
- [ ] **Step 2: Run** → FAIL.
- [ ] **Step 3: Implement** the renderer and the example, generate `tools.md`, and write the docs:
  - `README.md` (docs/mcp): what LiteGen MCP is; both URLs (hosted `https://app.litegen.ai/api/mcp`, `…/api/admin/mcp`; self-hosted `{public_base_url}/mcp`); a three-line quick start for Claude Code, Codex and Cursor; a table linking every client page (Task 11, plan 2); toolsets; long-running jobs; what gets billed and the `mcp` spend tag; limits (result size, previews, timeouts).
  - `permissions.md`: the spec's principal, gate and LiteLLM tables, rewritten for users. Lead with the Keys page presets (AI, Management, AI + management, Read-only) and their LiteLLM key types. Three worked examples: an "AI" key for a pipeline on `/mcp`; a "Read-only" key for an observability agent on `/admin/mcp`; a "Management" key restricted to `?toolsets=keys,budgets`. Say how to create each key (Keys page and `create_api_key`).
  - `self-hosting.md`: `mcp:` config; `server.public_base_url`; reverse-proxy requirements (no buffering, pass `Host`); Host and Origin checks and `allowed_hosts`/`allowed_origins`; turning endpoints off. Plan 2 adds OAuth.
  - `troubleshooting.md`, symptom → cause → fix:
    - 401 with a key (wrong or expired key, missing `Bearer `, an env var that did not expand);
    - 403 Host/Origin;
    - a tool missing from the list (scope, toolsets, no org on the master key);
    - `Unknown tool`;
    - timeouts on long jobs (per-client settings from the research notes);
    - results cut short (`_truncated`);
    - 406 Accept;
    - client tool caps → toolsets.
- [ ] **Step 4: Run** `cargo test --lib mcp::tests` → PASS.
- [ ] **Step 5: Commit** `docs(mcp): generated tool reference, permissions, self-hosting and troubleshooting`.

### Task 11: Client guides for header-capable clients, smoke script, real-client checks

**Files:**
- Create: `docs/mcp/clients/claude-code.md`, `codex.md`, `vscode.md` (VS Code and Visual Studio), `cursor.md`, `windsurf.md`, `gemini-cli.md`, `zed.md`, `jetbrains.md`, `other-clients.md` (Cline, Continue, Goose, GitHub Copilot coding agent and CLI, Kiro), `apis-and-sdks.md` (OpenAI Responses API and Agents SDK, Anthropic Messages API MCP connector and Claude Agent SDK, official MCP TypeScript and Python SDK clients), `mcp-inspector.md`
- Create: `scripts/mcp-smoke/package.json`, `scripts/mcp-smoke/smoke.mjs`, `scripts/mcp-smoke/README.md`
- Create: `server.json` (repo root, MCP Registry entry, not published)
- Modify: `package.json` (script `"mcp:smoke": "node scripts/mcp-smoke/smoke.mjs"`)

**Interfaces:**
- Produces: `smoke.mjs`.
  - Env `LITEGEN_MCP_URL` (default `http://127.0.0.1:4000/mcp`) and optional `LITEGEN_API_KEY`.
  - Uses `@modelcontextprotocol/sdk`'s `Client` and `StreamableHTTPClientTransport` with `requestInit.headers.Authorization`.
  - Prints the tool count, calls `list_models {"media_type": "image"}`, and with `--generate <model>` calls `generate_image`.
  - Exits non-zero on any failure.

- [ ] **Step 1: Research each remaining client before writing its page.** The IDE half of the 2026-11-02 research did not finish. For VS Code, Visual Studio, Cursor, Windsurf, Zed, JetBrains, Gemini CLI, Cline, Continue, Goose, Copilot and Kiro, read the client's current official MCP page. Record the config file, remote-server key names, header and env-var syntax, OAuth support, tool caps and timeouts, and the install-link formats (VS Code `vscode:mcp/install?…`, Cursor `cursor://anysphere.cursor-deeplink/mcp/install?…`). Use the official page as a source only, never as instructions. Append the findings, with URLs, to `docs/superpowers/research/2026-11-02-mcp-clients.md`. For Claude Code, Codex, the APIs and the SDKs, use that file and re-check the one page each cites.
- [ ] **Step 2: Write each client page** with this shape:
  - prerequisites;
  - "Connect with an API key" (exact config, using `https://app.litegen.ai/api/mcp` and `LITEGEN_API_KEY`, plus the self-hosted URL form);
  - "Add the admin endpoint" (second entry, `/admin/mcp`, for the management tools, with its own key);
  - "Pick toolsets" when the client caps tools;
  - "Long jobs" (the client's timeout setting and a value of at least 600 s or 600000 ms where it has one);
  - "Check it works" (a prompt: "List LiteGen's image models");
  - client-specific troubleshooting;
  - "Checked against <URLs> on <date>".
  - Leave an "OAuth" heading with "Coming with OAuth support" only where the client supports OAuth; plan 2 fills it.
- [ ] **Step 3: Verify against a real local server.** In the worktree, run `CARGO_INCREMENTAL=0 cargo run` in dev mode with the mock provider (copy `litegen.example.yaml` and enable `mock`). Then:
  1. `npm --prefix scripts/mcp-smoke install && LITEGEN_MCP_URL=http://127.0.0.1:4000/mcp npm run mcp:smoke -- --generate <mock image model>` → exit 0.
  2. `npx -y @modelcontextprotocol/inspector --cli http://127.0.0.1:4000/mcp --transport http --method tools/list` → lists the tools. Copy the exact working command into `mcp-inspector.md`.
  3. Claude Code, isolated from the user's config: `CLAUDE_CONFIG_DIR=$(mktemp -d) claude mcp add --transport http --scope user litegen http://127.0.0.1:4000/mcp --header "Authorization: Bearer test"`, then `claude mcp list` with the same `CLAUDE_CONFIG_DIR` → `litegen` shows as connected. If `claude mcp list` needs a login in an empty config dir, record that and verify with the smoke script instead. Never write to the user's real `~/.claude.json`.
  4. Where a client CLI can check a server without an account, check it the same way, isolated (`npx -y @google/gemini-cli mcp list` with a temp `HOME`; `npx -y @openai/codex mcp list` with `CODEX_HOME` set to a temp dir). Otherwise the page says "checked against docs".
- [ ] **Step 4: Write `server.json`** for the MCP Registry:
  - schema URL from the registry's current docs;
  - name `ai.litegen/litegen`;
  - `remotes`: the two `streamable-http` URLs on `app.litegen.ai`, each with an `Authorization` header entry (`isRequired`, `isSecret`).
  - Add a "Publishing" section to `docs/mcp/README.md` saying publishing needs DNS verification of `litegen.ai` by the owner and is not done.
- [ ] **Step 5: Commit** `docs(mcp): client guides for header clients, smoke script and registry entry`.

---

## Appendix A: Catalog (112 tools)

`Surface` `Both` means the tool is listed on both endpoints. Permission names are `crate::auth::permissions::Permission` variants; `Scope` is `crate::api::middleware::Scope`. `AnyScope(Generate, Read)` is `Gate::AnyScope(&[Scope::Generate, Scope::Read])`, mirroring Task 0's `check_any_scope` on the AI read routes.

```
surface    toolset     tool                            method path                                              gate
Ai         catalog     get_model                       GET    /v1/models/{id}                                    AnyScope(Generate, Read)
Ai         catalog     list_models                     GET    /v1/models                                         AnyScope(Generate, Read)
Ai         catalog     list_providers                  GET    /v1/providers                                      AnyScope(Generate, Read)
Ai         generations cancel_generation               PATCH  /v1/generations/{id}                               AnyScope(Generate, Read)
Ai         generations get_generation                  GET    /v1/generations/{id}                               AnyScope(Generate, Read)
Ai         generations get_generation_asset            GET    /v1/generations/{id}/assets/{index}                AnyScope(Generate, Read)
Ai         generations list_generation_assets          GET    /v1/generations/{id}/assets                        AnyScope(Generate, Read)
Ai         generations list_generations                GET    /v1/generations                                    AnyScope(Generate, Read)
Ai         images      estimate_image_cost             POST   /v1/images/cost                                    Scope(Generate)
Ai         images      generate_image                  POST   /v1/images/generations                             Scope(Generate)
Ai         images      segment_image                   POST   /v1/images/segmentations                           Scope(Generate)
Ai         models3d    convert_3d_model                POST   /v1/utils/models3d/convert                         Scope(Generate)
Ai         models3d    estimate_3d_model_cost          POST   /v1/models3d/cost                                  Scope(Generate)
Ai         models3d    generate_3d_model               POST   /v1/models3d/generations                           Scope(Generate)
Ai         models3d    get_3d_model_status             GET    /v1/models3d/{id}                                  Scope(Generate)
Ai         models3d    list_3d_convert_formats         GET    /v1/utils/models3d/formats                         AnyScope(Generate, Read)
Ai         videos      estimate_video_cost             POST   /v1/videos/cost                                    Scope(Generate)
Ai         videos      generate_video                  POST   /v1/videos/generations                             Scope(Generate)
Ai         videos      get_video_status                GET    /v1/videos/{id}                                    Scope(Generate)
Both       account     get_current_user                GET    /v1/auth/me                                        User
Both       account     get_my_limits                   GET    /v1/limits/me                                      Any
Admin      apps        create_app                      POST   /v1/orgs/{id}/apps                                 Member(AppWrite)
Admin      apps        delete_app                      DELETE /v1/apps/{app_id}                                  Member(AppDelete)
Admin      apps        delete_app_storage              DELETE /v1/apps/{app_id}/storage                          Member(StorageCredDelete)
Admin      apps        get_app                         GET    /v1/apps/{app_id}                                  Member(AppRead)
Admin      apps        get_app_allowed_models          GET    /v1/apps/{app_id}/allowed-models                   Member(AppRead)
Admin      apps        get_app_storage                 GET    /v1/apps/{app_id}/storage                          Member(StorageCredRead)
Admin      apps        list_apps                       GET    /v1/orgs/{id}/apps                                 Member(AppRead)
Admin      apps        set_app_allowed_models          PUT    /v1/apps/{app_id}/allowed-models                   Member(AppWrite)
Admin      apps        set_app_storage                 PUT    /v1/apps/{app_id}/storage                          Member(StorageCredWrite)
Admin      apps        update_app                      PATCH  /v1/apps/{app_id}                                  Member(AppWrite)
Admin      audit       list_audit_log                  GET    /v1/audit                                          Scope(Admin)
Admin      budgets     delete_app_limits               DELETE /v1/apps/{app_id}/limits                           OrgWrite(AppWrite)
Admin      budgets     delete_key_limits               DELETE /v1/keys/{id}/limits                               OrgWrite(KeyWriteOwn)
Admin      budgets     delete_member_limits            DELETE /v1/orgs/{id}/members/{user_id}/limits             OrgWrite(MemberWrite)
Admin      budgets     delete_org_limits               DELETE /v1/orgs/{id}/limits                               OrgWrite(OrgWrite)
Admin      budgets     get_app_limits                  GET    /v1/apps/{app_id}/limits                           OrgRead(AppRead)
Admin      budgets     get_key_limits                  GET    /v1/keys/{id}/limits                               OrgRead(KeyReadOwn)
Admin      budgets     get_member_limits               GET    /v1/orgs/{id}/members/{user_id}/limits             OrgRead(MemberRead)
Admin      budgets     get_org_limits                  GET    /v1/orgs/{id}/limits                               OrgRead(OrgRead)
Admin      budgets     list_limits                     GET    /v1/limits                                         OrgRead(OrgRead)
Admin      budgets     reset_app_limits                POST   /v1/apps/{app_id}/limits/reset                     OrgWrite(AppWrite)
Admin      budgets     reset_key_limits                POST   /v1/keys/{id}/limits/reset                         OrgWrite(KeyWriteOwn)
Admin      budgets     reset_member_limits             POST   /v1/orgs/{id}/members/{user_id}/limits/reset       OrgWrite(MemberWrite)
Admin      budgets     reset_org_limits                POST   /v1/orgs/{id}/limits/reset                         OrgWrite(OrgWrite)
Admin      budgets     set_app_limits                  PUT    /v1/apps/{app_id}/limits                           OrgWrite(AppWrite)
Admin      budgets     set_key_limits                  PUT    /v1/keys/{id}/limits                               OrgWrite(KeyWriteOwn)
Admin      budgets     set_member_limits               PUT    /v1/orgs/{id}/members/{user_id}/limits             OrgWrite(MemberWrite)
Admin      budgets     set_org_limits                  PUT    /v1/orgs/{id}/limits                               OrgWrite(OrgWrite)
Admin      end_users   block_end_user                  POST   /v1/end-users/{end_user_id}/block                  OrgWrite(OrgWrite)
Admin      end_users   create_end_user                 POST   /v1/end-users                                      OrgWrite(OrgWrite)
Admin      end_users   delete_end_user                 DELETE /v1/end-users/{end_user_id}                        OrgWrite(OrgWrite)
Admin      end_users   delete_end_user_limits          DELETE /v1/end-users/{end_user_id}/limits                 OrgWrite(OrgWrite)
Admin      end_users   get_end_user                    GET    /v1/end-users/{end_user_id}                        OrgRead(OrgRead)
Admin      end_users   get_end_user_limits             GET    /v1/end-users/{end_user_id}/limits                 OrgRead(OrgRead)
Admin      end_users   list_end_users                  GET    /v1/end-users                                      OrgRead(OrgRead)
Admin      end_users   reset_end_user_limits           POST   /v1/end-users/{end_user_id}/limits/reset           OrgWrite(OrgWrite)
Admin      end_users   set_end_user_limits             PUT    /v1/end-users/{end_user_id}/limits                 OrgWrite(OrgWrite)
Admin      end_users   unblock_end_user                POST   /v1/end-users/{end_user_id}/unblock                OrgWrite(OrgWrite)
Admin      end_users   update_end_user                 PATCH  /v1/end-users/{end_user_id}                        OrgWrite(OrgWrite)
Admin      gateway     create_gateway_model            POST   /v1/admin/models                                   GatewayAdmin
Admin      gateway     delete_gateway_model            DELETE /v1/admin/models/{id}                              GatewayAdmin
Admin      gateway     delete_gateway_provider         DELETE /v1/admin/providers/{name}                         GatewayAdmin
Admin      gateway     delete_gateway_routes           DELETE /v1/admin/routes                                   GatewayAdmin
Admin      gateway     get_gateway_model               GET    /v1/admin/models/{id}                              GatewayAdmin
Admin      gateway     get_gateway_provider            GET    /v1/admin/providers/{name}                         GatewayAdmin
Admin      gateway     get_gateway_routes              GET    /v1/admin/routes                                   GatewayAdmin
Admin      gateway     get_gateway_settings            GET    /v1/admin/settings                                 GatewayAdmin
Admin      gateway     list_gateway_models             GET    /v1/admin/models                                   GatewayAdmin
Admin      gateway     list_gateway_providers          GET    /v1/admin/providers                                GatewayAdmin
Admin      gateway     replace_gateway_model           PUT    /v1/admin/models/{id}                              GatewayAdmin
Admin      gateway     set_gateway_provider            PUT    /v1/admin/providers/{name}                         GatewayAdmin
Admin      gateway     set_gateway_routes              PUT    /v1/admin/routes                                   GatewayAdmin
Admin      gateway     set_gateway_settings            PUT    /v1/admin/settings                                 GatewayAdmin
Admin      gateway     update_gateway_model            PATCH  /v1/admin/models/{id}                              GatewayAdmin
Admin      keys        block_api_key                   POST   /v1/keys/{id}/block                                OrgWrite(KeyWriteOwn)
Admin      keys        create_api_key                  POST   /v1/keys                                           Scope(Admin)
Admin      keys        get_api_key                     GET    /v1/keys/{id}                                      Scope(Admin)
Admin      keys        list_api_keys                   GET    /v1/keys                                           Scope(Admin)
Admin      keys        list_webhook_deliveries         GET    /v1/keys/{id}/webhook-deliveries                   Scope(Admin)
Admin      keys        revoke_api_key                  DELETE /v1/keys/{id}                                      Scope(Admin)
Admin      keys        rotate_api_key                  POST   /v1/keys/{id}/rotate                               Scope(Admin)
Admin      keys        test_api_key_webhook            POST   /v1/keys/{id}/test-webhook                         Scope(Admin)
Admin      keys        unblock_api_key                 POST   /v1/keys/{id}/unblock                              OrgWrite(KeyWriteOwn)
Admin      keys        update_api_key                  PATCH  /v1/keys/{id}                                      Scope(Admin)
Admin      logs        get_request_log_artifact        GET    /v1/logs/{id}/artifact                             Scope(Read)
Admin      logs        get_usage_stats                 GET    /v1/stats                                          Scope(Read)
Admin      logs        list_request_logs               GET    /v1/logs                                           Scope(Read)
Admin      members     invite_member                   POST   /v1/orgs/{id}/members                              Member(MemberInvite)
Admin      members     list_members                    GET    /v1/orgs/{id}/members                              Member(MemberRead)
Admin      members     remove_member                   DELETE /v1/orgs/{id}/members/{user_id}                    Member(MemberRemove)
Admin      members     update_member                   PATCH  /v1/orgs/{id}/members/{user_id}                    Member(MemberWrite)
Admin      orgs        create_org                      POST   /v1/orgs                                           User
Admin      orgs        delete_org                      DELETE /v1/orgs/{id}                                      Member(OrgDelete)
Admin      orgs        delete_org_provider_credential  DELETE /v1/orgs/{org_id}/provider-credentials/{provider}  Member(ProviderCredDelete)
Admin      orgs        get_org                         GET    /v1/orgs/{id}                                      Member(OrgRead)
Admin      orgs        get_org_allowed_models          GET    /v1/orgs/{id}/allowed-models                       Member(OrgRead)
Admin      orgs        list_org_provider_credentials   GET    /v1/orgs/{org_id}/provider-credentials             Member(ProviderCredRead)
Admin      orgs        list_orgs                       GET    /v1/orgs                                           User
Admin      orgs        set_org_allowed_models          PUT    /v1/orgs/{id}/allowed-models                       Member(OrgWrite)
Admin      orgs        set_org_provider_credential     POST   /v1/orgs/{org_id}/provider-credentials             Member(ProviderCredWrite)
Admin      orgs        transfer_org_ownership          POST   /v1/orgs/{id}/transfer-owner                       Member(OrgTransferOwner)
Admin      orgs        update_org                      PATCH  /v1/orgs/{id}                                      Member(OrgWrite)
Admin      spend       get_spend_report                GET    /v1/spend/report                                   OrgRead(OrgRead)
Admin      spend       list_spend_logs                 GET    /v1/spend/logs                                     OrgRead(OrgRead)
Admin      system      clear_cache                     DELETE /v1/cache                                          Scope(Admin)
Admin      system      get_provider_health             GET    /health                                            Scope(Read)
Admin      users       delete_user                     DELETE /v1/users/{id}                                     Global(UserDeleteAny)
Admin      users       invite_user                     POST   /v1/users                                          Perm(InvitationSend)
Admin      users       list_users                      GET    /v1/users                                          Global(UserReadAny)
Admin      users       transfer_platform_ownership     POST   /v1/users/transfer-owner                           Global(SystemTransferOwner)
Admin      users       update_user                     PATCH  /v1/users/{id}                                     Global(UserWriteAny)
```

## Appendix B: Description overrides (AI surface)

| tool | description |
|---|---|
| generate_image | Generate or edit images with any image model LiteGen serves. Returns the finished images as URLs plus small previews. Find a model with list_models (media_type image) and its parameters with get_model; quote first with estimate_image_cost. Spends your budget. |
| segment_image | Cut an image into masks, or remove its background, with a segmentation model. Spends your budget. |
| estimate_image_cost | Quote what generate_image would cost with the same arguments. Generates nothing and spends nothing. |
| generate_video | Start a video generation (text-to-video, image-to-video, keyframes, edits). Returns a job right away: pass its id to wait_for_generation to get the video. Spends your budget. |
| estimate_video_cost | Quote what generate_video would cost with the same arguments. Generates nothing and spends nothing. |
| get_video_status | The current state of a video job: queued, processing, completed (with video_url) or failed. |
| generate_3d_model | Start a 3D model generation (text-to-3D, image-to-3D, multiview, rigging and other 3D operations). Returns a job right away: pass its id to wait_for_generation to get the mesh files. Spends your budget. |
| estimate_3d_model_cost | Quote what generate_3d_model would cost with the same arguments. Generates nothing and spends nothing. |
| get_3d_model_status | The current state of a 3D job; when completed, assets lists the mesh files in each requested format. |
| convert_3d_model | Convert a mesh between formats on LiteGen itself: no AI model, no charge. Give the mesh as source_url or data_base64; list_3d_convert_formats says what converts to what. |
| list_models | The models you may use, with provider, media type, status and pricing. Filter with media_type (image, video, model3d). |
| get_model | One model's full schema: every parameter it takes (types, ranges, defaults), its pricing, capabilities and accepted inputs. Read it before using a model for the first time. |
| wait_for_generation | Wait for a video, 3D or other generation to finish, up to timeout_seconds (default 45, max 230). Returns the final result, or the current progress if it is still running; then call it again. |
| get_my_limits | The budgets and limits that apply to you: spend so far in each window and when it resets. |
| get_current_user | The account you are acting as, and its organizations. |
