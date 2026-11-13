# Generation Call Hooks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operator-written Python, Node or Bun hook files, named in
`litegen.yaml`. On litegen's generation routes, `pre_call` runs before
authentication and `post_call` runs on the finished response.

**Architecture:**
- **Processes.** Each hook file runs in its own long-lived child process. The
  process runs an embedded runner (`runner.py`, or `runner.mjs` for Node and
  Bun) and talks to litegen in JSON lines over stdin/stdout.
- **`HookSet`.** It owns the processes, applied in order.
- **Placement.** Two axum layers wrap only the `Generate`-scope route group:
  `pre_call` outside auth, `post_call` inside it.
- **No hooks configured.** Then the group gets the plain auth layer and
  behaves exactly as today.

**Tech Stack:** Rust (axum 0.8, tokio `process`, tower-http 0.6 CORS),
Python 3 (asyncio), Node 24 / Bun (ESM), Playwright 1.60.

**Spec:** `docs/superpowers/specs/2026-11-02-generation-call-hooks-design.md`

## Global Constraints

- **Routes.** Hooks run only on the routes in `create_router`'s
  `generate_routes` group (the `Generate` scope). Management, read and auth
  routes never reach a hook.
- **Order.** `pre_call` runs before auth; `post_call` runs inside auth.
  Several hooks run in YAML order, and the first refusal stops the chain.
- **Runtimes.** `.py` → `python` (`python3`); `.js`/`.mjs`/`.cjs` → `node`
  (or `bun`); `.ts`/`.mts` → `bun` (or `node`). `runtime:` overrides the
  extension, and `command:` overrides the executable.
- **Paths and environment.** Hook paths resolve relative to litegen's working
  directory, where `litegen.yaml` is read. The hook process inherits
  litegen's environment plus the hook's `env:` map.
- **Time limit.** `timeout_ms` defaults to 2000.
- **Failure policy.** `on_error` unset: a `pre_call` failure refuses with
  503, and a `post_call` failure sends litegen's response unchanged.
  `on_error: refuse` or `on_error: allow` applies that policy to both phases.
- **Refusals:**
  - Python: an exception with `status_code`, which also carries `detail` and
    optionally `headers`.
  - Node and Bun: an `Error` with `status` or `statusCode`, and optionally
    `headers`.
  - A returned string refuses with 400.
- **Error body:** `{"error": {"type": "hook_error", "code":
  "refused_by_hook" | "hook_failed", "message": …, "hook": "<file name>"}}`.
  A refusal keeps the hook's status and headers; a failure is 503.
- **CORS.** Request headers are mirrored (`AllowHeaders::mirror_request()`).
  Credentials are never set together with a `*` origin. Exposed headers are
  unchanged.
- **Startup.** litegen refuses to start when:
  - a hook file is missing, fails to load, or defines neither function;
  - `hooks:` doesn't parse;
  - `hooks:` is non-empty but the rest of the config failed to load.
- **Repo rules:**
  - **No commits.** The user commits; each task ends with green tests and a
    ledger line.
  - **No parallel builds.** Every cargo command runs with
    `CARGO_INCREMENTAL=0`, and never two at once.
  - **`gate.sh` needs Postgres.** Start the container first:
    `docker run -d --rm --name litegen-b0-pg -e POSTGRES_PASSWORD=pw -e
    POSTGRES_DB=litegen_test -p 127.0.0.1:55439:5432 postgres:16-alpine`,
    and wait for `pg_isready`. Afterwards, run `docker stop litegen-b0-pg`.
  - **Rebuild before browser tests.** Any change to the Rust code needs
    `cargo build --release --bin litegen` before a Playwright run, because
    both Playwright configs run the release binary.
  - **Out of scope:** `src/mesh/` and `openapi.rs` aren't touched; the
    OpenAPI doesn't enumerate error codes.

## Review Focus

1. **A hook that prints** (Python `print`, `console.log`) must not corrupt
   the protocol. The runners point the hook's stdout at stderr. Pinned in
   Task 2 and Task 3 by `a_hook_that_prints_still_answers`.
2. **Many concurrent calls through one process,** answered out of order,
   must each get their own answer. Pinned in Task 2 and Task 3 by
   `concurrent_calls_are_matched_to_their_own_answers`.
3. **A large body,** a 5 MB inline image in JSON or multipart, must pass
   byte-identical through a hook that returns nothing. Pinned in Task 4 by
   `a_large_body_passes_through_an_untouched_hook_intact`.
4. **Repeated or non-UTF-8 header values and cookies** must reach litegen
   unchanged when the hook returns nothing. A `None` result forwards the
   original request without re-encoding. Pinned in Task 4 by
   `headers_the_hook_did_not_touch_reach_litegen_unchanged`.
5. **A malformed answer** must be a `hook_failed` 503 naming the hook, never
   a panic. That covers a number, a list, a changed `method` or `path`, or a
   non-string header value. Pinned in Task 4 by
   `a_malformed_answer_is_a_hook_failure`.

---

## Shared shapes (every task uses these names)

**The request a hook sees (JSON):**

```json
{ "method": "POST", "path": "/v1/images/generations", "query": "a=1",
  "headers": { "x-acme-token": "…" },
  "body": { … } | null,
  "files": [ { "name": "image_0", "filename": "a.png", "content_type": "image/png", "size": 1234 } ] }
```

- **`headers`:** names are lowercase, and repeated values are joined with
  `", "`. Non-UTF-8 values are left out of this view but kept on the
  request.
- **`body` and `files` by route:**
  - a JSON body, or a multipart `request` part, appears as `body`;
  - a multipart call's other parts are listed in `files` (otherwise
    `files` is `[]`);
  - the converter's raw upload and `GET`s give `body: null`.

**The response `post_call` sees:**
`{ "status": 200, "headers": { … }, "body": { … } | null }`. `body` is
`null` when the response isn't JSON. `post_call` sees every response the
handler produces, validation errors included, so a hook checks
`response.status`. Auth refusals never reach it, because they are answered
outside the `post_call` layer.

**The caller `post_call` sees:**
`{ "key_id": "uuid"|null, "org_id": …, "app_id": …, "end_user": …, "user":
{ "id", "email", "role" }|null }`. These come from the request's
`KeyContext`, and `end_user` from `x-litegen-end-user`.

**Hook signatures:**
- Python: `pre_call(request)` and `post_call(request, response, caller)`,
  each sync or `async`.
- Node and Bun: `export function pre_call(request)` and
  `export function post_call(request, response, caller)`, each sync or
  `async`.
- A hook returns `None`/`undefined` (no change), the full object changed,
  a string (a 400 refusal), or raises a status-carrying error.
- `pre_call` may change `headers`, `query` and `body`. A changed `method`
  or `path` is a malformed answer. `post_call` may change `status`,
  `headers` and `body`; `body` changes are ignored for non-JSON responses.

**Protocol (JSON lines):**
- Startup: runner → litegen
  `{"ready": true, "hooks": ["pre_call", "post_call"]}`, or
  `{"ready": false, "error": "…"}` and the runner exits.
- Call: litegen → runner `{"id": 7, "fn": "pre_call", "args": [ … ]}`.
- Answer: runner → litegen, one of:
  - `{"id": 7, "result": <value|null>}`
  - `{"id": 7, "refuse": {"status": 401, "message": "…", "headers": {…}}}`
  - `{"id": 7, "error": "<message and traceback>"}`

**Test fixtures.** `litegen-core/tests/fixtures/hooks/behaviour.{py,mjs,ts}`
behave identically, keyed on request headers.

- **`x-test-action`** (`pre_call`): a comma-separated list of actions,
  applied in order to the same request. The request is returned changed
  when any action changed it.

  | value | the hook |
  |---|---|
  | absent | returns `None` |
  | `inject` | sets `authorization: Bearer $TEST_LITEGEN_KEY` |
  | `refuse-status` | raises 401 `who are you` with `www-authenticate: Test` |
  | `refuse-string` | returns `not today` |
  | `rewrite-prompt` | appends ` (hooked)` to `body.prompt` |
  | `n2` | sets `body.n` to 2 |
  | `sleep` | waits 3 s, then returns `None` |
  | `sleep-300` | waits 300 ms |
  | `crash` | exits the process with code 3 |
  | `print` | prints `noise`, then returns `None` |
  | `bad-shape` | returns `42` |
  | `change-path` | sets `path` to `/v1/keys` |
  | `report` | raises 418 with the message `JSON.stringify({body, files, query, headers})` |
  | `env` | raises 418 with `$INHERITED_VAR` and `$HOOK_OWN_VAR` |

  `refuse_all.py` always raises 403 `refused by refuse_all`.

- **`x-test-post`** (`post_call`):

  | value | the hook |
  |---|---|
  | `tag` | sets `body.hooked` and header `x-hooked` to the runtime label |
  | `fail` | raises a plain error `boom` |
  | `report` | sets header `x-seen-caller` to the caller's JSON |
  | `teapot` | sets `status` to 418 |

- **Label.** Each file's runtime label is `python`, `node`, `bun` or
  `bun-ts`, read from `$HOOK_LABEL`.
- **Load-failure fixtures, each in `.py` and `.mjs`:** `neither`, `broken`
  (a syntax error) and `pre_only`.

---

### Task 0: Prerequisite — Bun on the test machine

**Files:** none.

- [ ] **Step 1:** with the user's OK, run `brew install oven-sh/bun/bun`.
- [ ] **Step 2:** run `python3 --version; node --version; bun --version`.
  Expected: Python ≥ 3.10, Node ≥ 22.18 (Node 24 is installed), and a Bun
  version printed.

### Task 1: Hook configuration and the startup guard

**Files:**
- Create: `litegen-core/src/hooks/mod.rs` (module docs and re-exports) and
  `litegen-core/src/hooks/config.rs`.
- Modify:
  - `litegen-core/src/lib.rs`: add `pub mod hooks;`;
  - `litegen-core/src/config/mod.rs:54`: add `#[serde(default)] pub hooks:
    Vec<crate::hooks::HookConfig>` to `AppConfig`;
  - `litegen-core/src/main.rs:29-32`: the load-failure guard.
- Test: unit tests in `config.rs`.

**Interfaces:**
- Produces:
  - `HookConfig { path: String, runtime: Option<Runtime>, command:
    Option<String>, timeout_ms: Option<u64>, on_error: Option<OnError>, env:
    BTreeMap<String, String> }`, with `Serialize` and `Deserialize`.
  - `enum Runtime { Python, Node, Bun }`, serialised as `python | node |
    bun`.
  - `enum OnError { Refuse, Allow }`.
  - `fn resolve(&self, base_dir: &Path) -> Result<ResolvedHook, String>`,
    returning `ResolvedHook { name: String /* file name */, path: PathBuf
    /* absolute */, runtime: Runtime, program: String, timeout: Duration,
    pre_policy: OnError, post_policy: OnError, env: BTreeMap<String,
    String> }`.
  - `pub fn hooks_in_config_file(dir: &Path) -> Result<Vec<HookConfig>,
    String>`. It reads only the `hooks` key of `dir/litegen.{yaml,yml,toml,
    json}`, and gives `Ok(vec![])` when there is no file or no key.

- [ ] **Step 1: Write the failing tests** in `config.rs` `#[cfg(test)]`:
  - `the_runtime_follows_the_extension_unless_named`: `a.py` → Python,
    `a.mjs` → Node, `a.ts` → Bun; `a.js` with `runtime: bun` → Bun.
  - `an_unknown_extension_without_a_runtime_is_an_error`: `a.rb` errors
    with the message `cannot tell which runtime runs a.rb; set runtime:`.
  - `the_path_resolves_against_the_base_dir_and_must_exist`: a missing
    file errors with the message `hook file not found: <abs path>`.
  - `defaults_are_2000_ms_refuse_before_allow_after`: `pre_policy ==
    Refuse`, `post_policy == Allow`, and `on_error: allow` sets both to
    `Allow`.
  - `command_overrides_the_program`: the default is `python3`, `node` or
    `bun`; `command: /opt/venv/bin/python` wins.
  - `hooks_in_config_file_reads_only_hooks`: a YAML with a broken
    `server:` but a valid `hooks:` gives both entries; a malformed `hooks:`
    gives `Err`.
- [ ] **Step 2:** run `cd litegen-core && CARGO_INCREMENTAL=0 cargo test
  --lib hooks::config`. Expected: it doesn't compile (`hooks` isn't
  defined).
- [ ] **Step 3: Implement** `config.rs` and the `AppConfig` field.
  - `hooks_in_config_file` uses `config::Config::builder()` with
    `File::with_name(dir.join("litegen"))`, then `get::<Vec<HookConfig>>
    ("hooks")`, mapping `NotFound` to an empty list.
  - In `main.rs`, when `load_config()` fails:
    - if `hooks_in_config_file(cwd)` is `Err`, or `Ok` with a non-empty
      list, print `litegen.yaml defines hooks but did not load: {e}` and
      exit 1;
    - otherwise keep today's fallback.
- [ ] **Step 4:** run the Step 2 command. Expected: 6 passed. Then run
  `CARGO_INCREMENTAL=0 cargo test --lib config` to check that the existing
  config tests still pass.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 2: Protocol, `HookProcess` and the Python runner

**Files:**
- Create:
  - `litegen-core/src/hooks/protocol.rs`;
  - `litegen-core/src/hooks/process.rs`;
  - `litegen-core/src/hooks/runners/runner.py`, embedded with
    `include_str!`;
  - the fixtures `litegen-core/tests/fixtures/hooks/behaviour.py`,
    `neither.py`, `broken.py` and `pre_only.py`.
- Test: `litegen-core/tests/hooks_runtime.rs`.

**Interfaces:**
- Consumes: `ResolvedHook` (Task 1).
- Produces:
  - `enum HookFn { PreCall, PostCall }`;
  - `enum HookOutcome { Unchanged, Replaced(serde_json::Value),
    Refused(Refusal) }`;
  - `struct Refusal { status: u16, message: String, headers: Vec<(String,
    String)> }`;
  - `enum HookCallError { Timeout, Crashed(String), Raised(String),
    Malformed(String) }`, with `Display`;
  - `HookProcess::start(hook: &ResolvedHook) -> Result<Arc<HookProcess>,
    String>`. It writes the runner to a per-process temp dir, spawns the
    program with `[runner, hook.path]` and inherited environment plus
    `hook.env`, and waits up to 10 s for `ready`.
  - `HookProcess::defines(&self, f: HookFn) -> bool`;
  - `HookProcess::call(&self, f: HookFn, args: Vec<serde_json::Value>,
    timeout: Duration) -> Result<HookOutcome, HookCallError>`. Answers are
    matched by id, so calls overlap.
  - **Restart.** A supervisor task respawns an exited child with backoff.
    In-flight calls get `Crashed`.
  - **No orphans.** The child is spawned with `kill_on_drop(true)`.
    Dropping the last `Arc<HookProcess>` stops the supervisor and kills the
    child. The runner also exits by itself when its stdin closes, so a hook
    never outlives litegen, even after a SIGKILL.
  - The runner is written to a private temp dir (mode 0700) that is
    removed on drop.

- [ ] **Step 1: Write the failing tests** in `hooks_runtime.rs`. Each runs
  over `RUNTIMES = [("python", behaviour.py)]` for now; Task 3 adds the
  rest. When the runtime's program is absent, the test prints
  `SKIPPED: <runtime> not installed` and returns.
  - `the_runner_reports_which_hooks_a_file_defines`: `pre_only` defines
    only `PreCall`.
  - `a_file_with_neither_hook_fails_to_start`: the error contains
    `defines neither pre_call nor post_call`.
  - `a_file_that_fails_to_load_fails_to_start`: the error contains
    `SyntaxError` (Python) or `SyntaxError` (JS).
  - `sync_and_async_hooks_both_answer`: in `behaviour`, `pre_call` is
    async and `post_call` is sync. Both give `Unchanged` for an empty
    action.
  - `a_raised_status_is_a_refusal_with_its_status_message_and_headers`:
    `Refused { status: 401, message: "who are you", headers:
    [("www-authenticate", "Test")] }`.
  - `a_returned_string_is_a_400_refusal`: `Refused { status: 400,
    message: "not today", .. }`.
  - `any_other_exception_is_a_hook_error_with_its_message`: the
    `x-test-post: fail` case gives `Raised(m)` where `m` contains `boom`.
  - `a_hook_that_prints_still_answers` (Review Focus 1).
  - `concurrent_calls_are_matched_to_their_own_answers` (Review Focus 2):
    20 calls at once, half of them `sleep`ing 300 ms (use a 2 s timeout
    and set `x-test-action: sleep-300`). Each gets its own answer.
  - `a_call_past_its_timeout_is_a_timeout_and_the_next_call_still_works`:
    a `sleep` call with a 200 ms limit gives `Timeout`, and the next call
    gives `Unchanged`.
  - `a_crashed_hook_is_restarted_and_answers_again`: `crash` gives
    `Crashed`, and within 5 s a call gives `Unchanged` again.
  - `the_hook_inherits_litegens_environment_plus_its_own_env`: `env`
    refuses with a message containing both values. Set `INHERITED_VAR` in
    the test's environment and `HOOK_OWN_VAR` in `hook.env`.
  - `a_hook_never_outlives_its_owner`:
    - after the `HookProcess` is dropped, the child's pid is gone within
      2 s (`kill -0` fails);
    - a runner whose stdin is closed exits on its own within 2 s.
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --test hooks_runtime`.
  Expected: it doesn't compile.
- [ ] **Step 3: Implement** `protocol.rs`, `process.rs` and `runner.py`.
  - The runner keeps a private dup of fd 1 for the protocol, then points
    fd 1 at fd 2, so hook output goes to stderr.
  - It loads the file with `importlib.util.spec_from_file_location`, with
    the file's directory on `sys.path`.
  - It runs `async def` hooks on its asyncio loop and sync ones through
    `asyncio.to_thread`.
  - It maps refusals and errors as in "Shared shapes".
  - The child's stderr goes to `tracing` at `info`, under target
    `litegen::hooks` with the hook name.
- [ ] **Step 4:** run the Step 2 command. Expected: all pass (python).
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 3: The JavaScript runner (Node, Bun, TypeScript)

**Files:**
- Create:
  - `litegen-core/src/hooks/runners/runner.mjs`, embedded;
  - the fixtures `behaviour.mjs`, `behaviour.ts`, `neither.mjs`,
    `broken.mjs` and `pre_only.mjs`.
- Modify: `hooks_runtime.rs`. Extend `RUNTIMES` to `python`,
  `node (.mjs)`, `bun (.mjs)` and `bun-ts (.ts)`, plus
  `node-ts (.ts, runtime: node)` when `node --version` ≥ 22.18.

**Interfaces:**
- Consumes: the protocol (Task 2).
- Produces: `runner.mjs` speaks the same protocol under both `node` and
  `bun`.

- [ ] **Step 1: Write the failing tests:** the Task 2 tests now run for
  each new runtime.
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --test hooks_runtime`.
  Expected: FAIL for node, bun and bun-ts (the runner is missing).
- [ ] **Step 3: Implement** `runner.mjs`.
  - It loads the hook with `await import(pathToFileURL(path))`.
  - It keeps the original `process.stdout.write` for the protocol, and
    sends `process.stdout.write`, `console.log`, `console.info` and
    `console.debug` to stderr.
  - It reads lines with `node:readline`, which Bun supports.
  - A thrown error with a numeric `status` or `statusCode` is a refusal;
    any other error is `error`.
- [ ] **Step 4:** run the Step 2 command. Expected: all pass for python,
  node, bun, bun-ts and node-ts.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 4: `HookSet`, the `pre_call` layer and the router

**Files:**
- Create:
  - `litegen-core/src/hooks/request.rs`, which builds the hook's request
    view and applies the answer;
  - `litegen-core/src/hooks/layer.rs`, with `pre_call_layer` and the error
    responses.
- Modify:
  - `hooks/mod.rs`: `HookSet`;
  - `litegen-core/src/api/handlers/mod.rs:3806`: `create_router_with_hooks`;
  - `litegen-core/src/api/mod.rs:8`: re-export it;
  - `litegen-core/tests/harness/mod.rs`: `app_with_hooks`.
- Test: `litegen-core/tests/hooks_api.rs`.

**Interfaces:**
- Consumes: `HookProcess` and `HookOutcome` (Task 2), and `ResolvedHook`
  (Task 1).
- Produces:
  - In `request.rs`:
    - `pub(crate) struct HookedRequest`: the original `http::request::Parts`
      plus the buffered body bytes;
    - `pub(crate) async fn hook_view(req: Request) -> Result<(HookedRequest,
      serde_json::Value), Response>`, which buffers the body and builds the
      view described in "Shared shapes";
    - `pub(crate) fn apply_answer(hooked: HookedRequest, answer:
      Option<serde_json::Value>) -> Result<Request, String>`. `None`
      rebuilds the identical request; `Err` is a malformed answer.
  - `HookSet::start(configs: &[HookConfig], base_dir: &Path) ->
    Result<Option<Arc<HookSet>>, String>`. It gives `None` when `configs`
    is empty, and an error naming the hook on any start failure.
  - `pub async fn pre_call_layer(State(hooks): State<Arc<HookSet>>, req:
    Request, next: Next) -> Response`;
  - `pub fn create_router_with_hooks(state: Arc<AppState>, hooks:
    Option<Arc<HookSet>>) -> axum::Router`. `create_router(state)` calls it
    with `None`.
  - Harness: `app_with_hooks(asset_dir: &Path, master_key: Option<&str>,
    hooks: Vec<HookConfig>) -> (Router, Arc<SqliteDatabase>)`. It sets
    `AppState.master_key`, and the hooks' base dir is
    `tests/fixtures/hooks`.
- Router shape:
  - With hooks:
    `generate_routes.layer(post_call_layer).layer(auth_layer.clone()).layer(pre_call_layer)`.
    Task 4 has no post layer yet.
  - Without hooks: `generate_routes.layer(auth_layer.clone())`.
  - Either way, the group is merged outside `authenticated_routes`, which
    no longer contains it.
- Body view by route:
  - JSON POST routes (generations, segmentations, cost): JSON, or the
    multipart `request` part plus `files`.
  - `/v1/utils/models3d/convert` and the `GET`s: `null`.
- An `Unchanged` result forwards the original request without
  re-encoding.

- [ ] **Step 1: Write the failing tests** in `hooks_api.rs`. Each runs over
  every installed runtime's `behaviour` file, with master key `test-master`
  and `TEST_LITEGEN_KEY=test-master`.
  - `an_injected_key_authenticates_a_caller_with_no_litegen_key`: `POST
    /v1/images/generations` with `mock/visual-image-gen` and `inject`, no
    `authorization` → 200.
  - `a_request_the_hook_leaves_alone_is_judged_by_litegens_own_auth`: no
    action and no key → 401; no action and `Bearer test-master` → 200.
  - `a_raised_status_reaches_the_caller_as_that_status_with_its_headers`:
    401, `www-authenticate: Test`, and the body's `error.code ==
    "refused_by_hook"`, `error.message == "who are you"` and `error.hook ==
    "behaviour.<ext>"`.
  - `a_returned_string_refuses_with_400`.
  - `a_rewritten_body_is_what_the_handler_validates_and_runs`: with
    `inject` plus `rewrite-prompt` (a header list
    `x-test-action: inject,rewrite-prompt`, which the fixtures support),
    the stored `GET /v1/generations/{id}` gives `metadata.request.prompt ==
    "a fox (hooked)"`.
  - `a_rewrite_cannot_skip_validation`: the hook sets `n: 2` (`x-test-action:
    inject,n2`) → 400 `param_out_of_range` from litegen.
  - `the_multipart_request_part_is_hooked_and_its_files_pass_through_intact`:
    `report` on a multipart call (a `request` part and a PNG part `image_0`)
    → 418, and the message's `files[0]` has `{name: "image_0",
    content_type: "image/png", size: <len>}` and `body.prompt == "a fox"`.
    Then `inject,rewrite-prompt` on the same multipart call → 200, and the
    stored `metadata.request.reference_images[0].sha256` equals the PNG's
    SHA-256.
  - `the_converter_route_shows_the_hook_headers_and_query_only`: `report`
    on `POST /v1/utils/models3d/convert?to=obj` with GLB bytes → 418, and
    the message's `body` is `null` and `query == "to=obj"`.
  - `a_timed_out_pre_call_refuses_with_503_hook_failed`: `timeout_ms: 200`
    with `sleep` → 503 and `error.code == "hook_failed"`.
  - `with_on_error_allow_a_failed_pre_call_lets_the_request_through`:
    `on_error: allow`, `sleep`, and `Bearer test-master` → 200.
  - `a_crashed_pre_call_refuses_then_the_restarted_hook_serves`: `crash`
    → 503, then within 5 s `inject` → 200.
  - `a_malformed_answer_is_a_hook_failure` (Review Focus 5): `bad-shape`
    and `change-path` each → 503 `hook_failed`, whose message names the
    hook.
  - `hooks_run_in_order_and_the_first_refusal_stops_the_chain`:
    - the chain `[behaviour.py, behaviour.mjs, behaviour.ts]`, run with
      `x-test-action: inject,rewrite-prompt`, stores a prompt ending
      ` (hooked) (hooked) (hooked)`;
    - the chain `[refuse_all.py, behaviour.mjs]`, run with `x-test-action:
      report`, answers 403 `refused by refuse_all`, not the second hook's
      418.
  - `management_and_read_routes_never_reach_a_hook`: with a
    `refuse-everything` hook (fixture `refuse_all.py`, which always raises
    403), `GET /v1/keys` and `GET /v1/models` with `Bearer test-master` →
    200.
  - `headers_the_hook_did_not_touch_reach_litegen_unchanged` (Review
    Focus 4), in two parts:
    - through the app: a request carrying a non-UTF-8 `x-raw` value, two
      `x-dup` headers and a cookie, with no action and `Bearer
      test-master`, answers 200;
    - a unit test in `request.rs`,
      `an_unchanged_answer_rebuilds_the_identical_request`: for that
      request, `apply_answer(hooked, None)` gives the same method, URI,
      headers (every value byte-for-byte, in order) and body bytes.
  - `a_large_body_passes_through_an_untouched_hook_intact` (Review
    Focus 3): a JSON image request with a 5 MB base64 `init` reference
    (`mock/mesh-3d` image-to-3D) and `Bearer test-master` → 200, and the
    stored reference's `bytes` equals the decoded length.
  - `#[ignore] pass_through_overhead` (the spec's ~1 ms target, measured
    rather than asserted): for each runtime, 200 sequential image-cost
    requests (`POST /v1/images/cost`) with a 10 KB body, with and without
    a no-op hook. It prints the p50 and p95 of the difference.
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --test hooks_api`.
  Expected: it doesn't compile.
- [ ] **Step 3: Implement** `request.rs`, `layer.rs` (pre), `HookSet` and
  the router changes.
  - Multipart: parse with `multer`. When the answer changed the body,
    rebuild the multipart with the same boundary, the new `request` part
    and the original file parts.
  - Drop hop-by-hop headers and recompute `content-length`.
  - Bound the buffering with a `tokio::sync::Semaphore` of
    `backpressure.max_in_flight` permits, taken before the body is read.
  - Read the body only when a hook defines `pre_call` and the route's body
    view isn't `null`. The converter's upload always streams through
    unread, and `GET`s have none. Buffered bodies keep the validator's
    25 MiB limit; over it is a 413, as today.
- [ ] **Step 4:** run the Step 2 command. Expected: all pass for every
  installed runtime. Then run
  `zsh <that session's scratchpad>/bin/gate.sh` (the scratchpad is gone).
  Expected: `TOTAL … failed 0`. The router regrouping must not change any
  existing test.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 5: The `post_call` layer

**Files:**
- Modify:
  - `hooks/layer.rs`: `post_call_layer`;
  - `api/handlers/mod.rs`: add `post_call_layer` inside auth;
  - `hooks_api.rs`.

**Interfaces:**
- Consumes: `HookSet` (Task 4).
- Produces: `pub async fn post_call_layer(State(hooks):
  State<Arc<HookSet>>, req: Request, next: Next) -> Response`. It reads
  `KeyContext` from the request's extensions, and `end_user` from
  `x-litegen-end-user`. It buffers only `application/json` responses, and
  passes other bodies through as a stream.

- [ ] **Step 1: Write the failing tests:**
  - `post_call_sees_the_finished_image_response_and_can_rewrite_it`:
    `tag` → `body.hooked == <label>` and header `x-hooked: <label>`.
  - `post_call_sees_the_authenticated_caller`: `report` with `Bearer
    test-master` and `x-litegen-end-user: alice` → `x-seen-caller` parses
    to `end_user == "alice"`, with `user == null` and no `key_id` (a
    master-key caller).
  - `post_call_sees_each_video_and_3d_status_response_including_the_finished_one`:
    poll `GET /v1/models3d/{id}` (with `mock/mesh-3d`) and `GET
    /v1/videos/{id}` (with the mock video model) with `tag`. Every answer
    carries `hooked`, including the `completed` one, whose assets are
    present.
  - `a_failing_post_call_sends_litegens_response_unchanged_by_default`:
    `fail` → 200 and the normal body, with no `hooked`.
  - `with_on_error_refuse_a_failing_post_call_answers_503`.
  - `post_call_can_change_the_status`: `teapot` → 418.
  - `post_call_sees_a_validation_error_response_too`: `report` on a request
    litegen refuses as invalid (`n: 2`) → 400, still with the
    `x-seen-caller` header.
  - `the_converter_response_streams_through_post_call_untouched`: with
    `tag`, the converted bytes are identical to a hook-free run, and the
    `x-hooked` header is present.
  - `an_idempotent_replay_passes_through_post_call_again`: the same
    `Idempotency-Key` twice with `tag` → both carry `hooked`, and the
    second has `idempotent-replayed: true`.
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --test hooks_api`.
  Expected: the new tests FAIL.
- [ ] **Step 3: Implement** `post_call_layer` and wire it.
- [ ] **Step 4:** run the Step 2 command, then `gate.sh`. Expected: all
  pass, `failed 0`.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 6: CORS and wiring the binary

**Files:**
- Modify:
  - `litegen-core/src/api/cors.rs:46-68` and its tests;
  - `litegen-core/src/main.rs:239-257`: start the hooks and route them.

**Interfaces:**
- Consumes: `HookSet::start` (Task 4) and `create_router_with_hooks`.
- Produces: `main` builds the router with
  `create_router_with_hooks(state, HookSet::start(&config.hooks,
  &cwd).await?)`. A start error prints `hook <name>: <error>` and exits 1.

- [ ] **Step 1: Write the failing tests** in `cors.rs`:
  - `a_browser_may_send_any_request_header`: a preflight with
    `access-control-request-headers: x-acme-token, authorization` → both
    are listed in `access-control-allow-headers`.
  - `credentials_are_never_allowed_with_a_wildcard_origin`: origins `["*"]`
    and `allow_credentials: true` → the layer builds without a panic, and
    a response has no `access-control-allow-credentials`.
  - The existing tests in `cors.rs` must still pass, including the
    preflight test that sends the SDK headers (the `let sent = [...]` list
    near line 110).
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --lib api::cors`.
  Expected: the 2 new tests FAIL.
- [ ] **Step 3: Implement** both.
  - Use `AllowHeaders::mirror_request()`.
  - With a `*` origin, skip `allow_credentials` and
    `warn!("cors: credentials are never allowed with a * origin")`.
  - Then wire `main.rs`.
- [ ] **Step 4:** run the Step 2 command, then `CARGO_INCREMENTAL=0 cargo
  build --bin litegen`. Expected: pass, and the build succeeds.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 7: The examples and the real-binary end-to-end test

**Files:**
- Create:
  - `examples/hooks/custom_auth.py` and `examples/hooks/custom_auth.mjs`:
    - read `x-acme-token`, and leave alone a request that already has
      `authorization`;
    - map tokens through `$ACME_TOKENS` (`token:user,…`);
    - on a match, set `authorization: Bearer $LITEGEN_APP_KEY` and
      `x-litegen-end-user: <user>`;
    - otherwise raise 401 with `www-authenticate: AcmeToken`.
  - `examples/hooks/rewrite.ts` (Bun TypeScript): map `body.model` through
    `{ "fast-image": "mock/visual-image-gen" }`.
  - `examples/hooks/tag_results.mjs` (`post_call`): on 2xx JSON, add
    `body.served_by = "litegen+hooks"` and the header `x-served-by`.
  - `examples/hooks/cookie_auth.py`:
    - read the cookie `acme_session=<token>`;
    - refuse 403 when the `origin` is set and isn't in `$ALLOWED_ORIGINS`;
    - otherwise inject the key as `custom_auth` does.
  - `litegen-core/tests/hooks_e2e.rs`.

**Interfaces:**
- Consumes: the binary (`env!("CARGO_BIN_EXE_litegen")`) and `main.rs`
  (Task 6).
- Produces: the examples the docs (Task 8) and the browser test (Task 9)
  use.

- [ ] **Step 1: Write the failing tests** in `hooks_e2e.rs`.
  - Each test sets up its own temp dir: a `litegen.yaml`, the examples
    copied into `hooks/`, and a free port.
  - It spawns the binary with `current_dir` set to that temp dir, plus
    `LITEGEN__SERVER__PORT`, `LITEGEN__DATABASE_URL=sqlite://:memory:`,
    `LITEGEN__MASTER_KEY=e2e-master`,
    `LITEGEN__PROVIDERS__MOCK__API_KEY=`, `LITEGEN_MODELS_DIR=<repo>/models`,
    `LITEGEN_APP_KEY=e2e-master` and `ACME_TOKENS=alice-token:alice`.
  - It waits for `/health`.
  - The tests:
    - `a_mixed_chain_authenticates_rewrites_and_tags_a_real_generation`:
      the chain is `[custom_auth.py, rewrite.ts, tag_results.mjs]`. With
      `x-acme-token: alice-token`, `model: "fast-image"` → 200,
      `served_by` is set, the stored request's model is
      `mock/visual-image-gen`, and the end user is `alice`.
    - `the_node_custom_auth_matches_the_python_one`: the same flow with
      `custom_auth.mjs`.
    - `an_unknown_token_gets_the_hooks_401`: 401 and
      `www-authenticate: AcmeToken`.
    - `video_and_3d_jobs_poll_to_completion_through_the_hooks`: `POST`,
      then poll `/v1/videos/{id}` and `/v1/models3d/{id}` with only
      `x-acme-token` → `completed`, with `served_by` set.
    - `the_multipart_and_converter_paths_work_through_the_hooks`:
      - a multipart `mock/mesh-3d` call (a `request` part and a PNG
        `image_0`) with only `x-acme-token` gives 200 and polls to
        `completed`;
      - `POST /v1/utils/models3d/convert?to=obj` with a GLB and only
        `x-acme-token` gives 200, with an OBJ body and `x-served-by` set.
    - `management_routes_ignore_the_hooks`: `GET /v1/keys` with only
      `x-acme-token` → 401, because no hook injected a key there; with
      `Bearer e2e-master` → 200.
    - `a_timeout_and_a_crash_refuse_and_the_crash_recovers`: the chain is
      `[custom_auth.py, behaviour.py (timeout_ms: 200), behaviour.mjs]`.
      - `x-test-action: sleep` gives 503 `hook_failed` naming
        `behaviour.py`;
      - `x-test-action: crash` gives 503;
      - within 5 s, a request with only `x-acme-token` gives 200.
    - `litegen_refuses_to_start_on_a_missing_or_broken_hook`: a missing
      file, `broken.py`, a malformed `hooks:`, and valid `hooks:` beside a
      broken `server:`. Each exits non-zero within 10 s, with the hook's
      name or `defines hooks but did not load` in its stderr.
- [ ] **Step 2:** run `CARGO_INCREMENTAL=0 cargo test --test hooks_e2e`.
  Expected: FAIL (the examples are missing).
- [ ] **Step 3: Implement** the five example files.
- [ ] **Step 4:** run the Step 2 command. Expected: all pass.
- [ ] **Step 5:** append a ledger line. Don't commit.

### Task 8: Documentation

**Files:**
- Create: `docs/hooks.md`.
- Modify: `litegen.example.yaml` (a commented `hooks:` block) and
  `README.md` (a "Hooks" section linking to the guide).

- [ ] **Step 1: Write `docs/hooks.md`.** Its sections:
  1. what hooks are, and how they compare with LiteLLM, from the spec's
     "Better than LiteLLM" section;
  2. configuration (every option, defaults, the runtime table);
  3. the objects (request, response and caller field tables);
  4. Python, Node and Bun TypeScript side by side for each example in
     `examples/hooks/`, quoting those files;
  5. refusing vs failing;
  6. which routes are hooked;
  7. the trust model;
  8. the CSRF duty of cookie hooks;
  9. deploying (the runtime in the image).

  Each example code block sits under a marker line,
  `<!-- example: examples/hooks/<file> -->`, and quotes that file whole.
- [ ] **Step 2: Write the failing test**
  `the_guide_quotes_every_example_verbatim` in `hooks_e2e.rs`:
  - every marker's fenced block equals its file's contents exactly;
  - each of the five example files has a marker.

  Run `CARGO_INCREMENTAL=0 cargo test --test hooks_e2e
  the_guide_quotes_every_example_verbatim`. Expected: FAIL (no guide yet).
- [ ] **Step 3:** write the guide, then run the Step 2 command. Expected:
  PASS.
- [ ] **Step 4:** append a ledger line. Don't commit.

### Task 9: The browser test and the final proof

**Files:**
- Create:
  - `dashboard/playwright.hooks.config.ts`. Its web server is the litegen
    binary on port 5199, with `cwd` set to `dashboard/e2e-hooks/fixture`.
    It sets `LITEGEN_CORS_ORIGINS=http://app.acme.test`,
    `LITEGEN__CORS__ALLOW_CREDENTIALS=true`, a master key, `ACME_TOKENS`,
    `LITEGEN_APP_KEY` and `ALLOWED_ORIGINS=http://app.acme.test`. Video is
    on and `slowMo` is 250.
  - `dashboard/e2e-hooks/fixture/litegen.yaml`, whose hooks are
    `../../../examples/hooks/custom_auth.py` and `cookie_auth.py`.
  - `dashboard/e2e-hooks/custom-auth.spec.ts`.

- [ ] **Step 1: Write the failing spec:**
  - `a page on another origin generates through the custom-auth hook`:
    - `page.route('http://app.acme.test/**')` serves a page whose script
      `fetch`es `http://127.0.0.1:5199/v1/images/generations` with
      `x-acme-token: alice-token` and a JSON body;
    - the result is status 200 with an image;
    - with the token `wrong`, it is 401.
  - `the preflight lets the custom header through`: an `OPTIONS` request
    with `Origin: http://app.acme.test` and
    `Access-Control-Request-Headers: x-acme-token, content-type` →
    `access-control-allow-headers` contains `x-acme-token`.
  - `the cookie hook refuses a cross-site request`: an API-context `POST`
    with `cookie: acme_session=alice-token` and `origin:
    https://evil.test` → 403; with `origin: http://app.acme.test` → 200.
- [ ] **Step 2:** rebuild the release binary the browser tests run
  (`cd litegen-core && CARGO_INCREMENTAL=0 cargo build --release --bin
  litegen`), then run `cd dashboard && npx playwright test --config
  playwright.hooks.config.ts`. Expected: FAIL until the config and the
  fixture exist.
- [ ] **Step 3:** create the config and the fixture.
- [ ] **Step 4: Final proof.** Run, in turn:
  - `gate.sh` (the full Rust suite with Postgres), expecting `failed 0`;
  - `cargo test --test hooks_runtime --test hooks_api --test hooks_e2e`,
    expecting all pass for python, node, bun and bun-ts;
  - `npx playwright test --config playwright.hooks.config.ts`, expecting
    all pass;
  - the main dashboard e2e suite (`npx playwright test`), expecting 31
    passed and unchanged;
  - `CARGO_INCREMENTAL=0 cargo test --release --test hooks_api --
    --ignored pass_through_overhead --nocapture`, and report each
    runtime's p50 and p95 against the spec's ~1 ms target.
- [ ] **Step 5:** append a ledger line. Don't commit. Report to the user
  with the `open` command for the new videos.
