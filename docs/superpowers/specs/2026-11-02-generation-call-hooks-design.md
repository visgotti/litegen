# Generation call hooks — design

Status: approved 2026-11-02 ("yes, but make sure ours is better: Node/Bun or
Python; update the documentation with examples; thorough end-to-end testing
proving everything works"). Nothing is built yet.

## Intent

An operator can put their own code in front of, and behind, litegen's AI
generation calls. LiteLLM's proxy does the same with `custom_auth`, an
`async_pre_call_hook` and an `async_post_call_success_hook`, loaded from files
named in its `config.yaml`. litegen follows LiteLLM wherever the two overlap
(see "Following LiteLLM" below). The hook is a **Python or Node file whose
path is set in litegen's YAML**.

The motivating case is **custom per-user authentication**. A customer's
callers should not have to hold a litegen API key. They send whatever the
customer's own system issues: a JWT from its identity provider, a session
cookie or a custom header. The pre hook checks that credential its own way
and **injects the litegen API key** into the request. litegen then
authenticates, authorizes and validates the request exactly as it always
does. If the hook leaves a request alone, litegen's own auth judges what the
caller sent. That is LiteLLM's `custom_auth_settings: mode: auto`: custom
auth first, then the built-in key auth, then 401.

The post hook sees litegen's finished response, with the generation's assets
and URLs filled in, and can alter it or act on it before it goes back to the
caller.

## Goals

1. **Configured in YAML, in Python, Node or Bun.** `hooks:` in
   `litegen.yaml` lists one or more hook files. Each path is resolved relative
   to the directory of `litegen.yaml`, as LiteLLM resolves its callback files
   relative to `config.yaml`. Each hook runs on one of three runtimes, chosen
   by `runtime:` or, when that is left out, by the file's extension:

   | extension | default runtime | also allowed |
   |---|---|---|
   | `.py` | `python` (`python3`) | — |
   | `.js`, `.mjs`, `.cjs` | `node` | `bun` |
   | `.ts`, `.mts` | `bun` | `node` (Node 23.6+ strips types itself) |

   `command:` overrides the executable, for a venv's Python or a pinned Node
   or Bun binary. A single JavaScript runner serves both Node and Bun.
2. **Two hooks per file, both optional.** A file defines `pre_call` and/or
   `post_call`, each sync or async in Python and sync or async in Node.
3. **`pre_call` runs before authentication,** on the raw incoming request.
   - It receives the method, path, query and headers, and the body when
     there is one it can read (see "What a hook sees").
   - It can:
     - return the request changed: headers added, removed or replaced, and
       the body changed;
     - return nothing, leaving the request untouched;
     - refuse it, the way LiteLLM's hooks do (see "Refusing a request").
   - The changed request then goes through litegen's usual pipeline:
     authentication, scope check, CSRF, idempotency, backpressure,
     validation and the handler.
4. **`post_call` runs on the response.** It runs after the handler has
   produced the full response and before it is sent. It receives:
   - the request as litegen handled it (after any `pre_call` change);
   - the response's status, headers and body;
   - the caller as litegen authenticated it: key id, org, app and end user.
     This is LiteLLM's `user_api_key_dict`.

   It can return the response changed, return nothing, or refuse.
5. **Only the generation routes.** Hooks run on exactly the routes in
   litegen's `Generate` scope group (`create_router`'s `generate_routes`):
   - `POST /v1/images/generations` and `POST /v1/images/segmentations`;
   - `POST /v1/videos/generations` and `GET /v1/videos/{id}`;
   - `POST /v1/models3d/generations` and `GET /v1/models3d/{id}`;
   - the three `POST …/cost` quotes;
   - `POST /v1/utils/models3d/convert`;
   - any route added to that group later.

   Image generation answers in one call, so its `post_call` sees the finished
   images. Video and 3D answer `pending` first, so `post_call` sees every
   status response, including the one that carries the finished result.
6. **Several hooks, in order.** `pre_call`s run top to bottom, each seeing the
   previous one's output, and the first refusal stops the chain. `post_call`s
   run top to bottom too.

## Better than LiteLLM

These are deliberate. Each needs a test that proves it.

- **Three runtimes, not one.** Hooks run in Python, Node or Bun, and Bun
  runs TypeScript directly. LiteLLM takes Python only.
- **One chain, any mix of languages.** A Python auth hook can run first, with
  a Bun rewrite after it.
- **A hook can't take the gateway down.** Each hook runs in its own process.
  A crash, an infinite loop or a memory blow-up stays in that process:
  litegen refuses (or, if configured, passes) the affected calls and
  restarts the hook. LiteLLM's callbacks run inside the proxy.
- **A time limit and a failure policy per hook.** LiteLLM's hooks have
  neither.
- **Custom auth and request rewriting in one place.** The pre hook runs
  before auth. One function can authenticate the caller and rewrite the
  request, where LiteLLM splits them between `custom_auth` and the pre-call
  hook.
- **The finished result of async jobs.** For video and 3D, `post_call` sees
  the status response that carries the finished assets.
- **Sync or async functions in every language.**

## Documentation and examples (deliverables)

- **`docs/hooks.md`, the operator's guide.** It covers:
  - the YAML and every option;
  - the request and response objects each hook gets, with a field table;
  - refusing and failing;
  - the trust model;
  - the CSRF duty of cookie-based hooks;
  - which routes are hooked;
  - Python, Node and Bun side by side.
- **Example hook files under `examples/hooks/`.** Each one is runnable and
  each is exercised by the tests:
  - custom auth that checks the caller's own token and injects a litegen
    key, in Python and in Node;
  - a request rewrite in Bun TypeScript, for example defaulting a param or
    aliasing a model;
  - a `post_call` that tags or rewrites a finished result, in Node;
  - cookie auth with an `Origin` check, in Python.
- **A commented `hooks:` example in `litegen.example.yaml`,** and a README
  section that links to the guide.

## Proof (deliverables)

- **Integration tests on real hook processes in every runtime,** run against
  the in-process router. Every behavior in this design is tested once per
  runtime (Python, Node, Bun), including TypeScript under Bun. A runtime that
  is absent is skipped with a loud message, but this machine runs all three.
- **An end-to-end test on the real binary.** It starts the `litegen` binary
  (`CARGO_BIN_EXE_litegen`) with a real `litegen.yaml` declaring hooks in all
  three runtimes, and drives it over HTTP:
  - custom auth injects a key, and a mock generation runs, is polled to
    completion and is rewritten by `post_call`;
  - both refusal forms;
  - a timeout, a crash and the restart;
  - the multipart and converter paths;
  - the management routes, untouched;
  - paths resolved relative to `litegen.yaml`, and the environment
    inherited.
- **A browser test.** A Playwright spec with its own config boots litegen
  with an auth hook. A page on another origin calls a generation route with a
  custom auth header, proving that the CORS preflight passes and that the
  hook's injected key is used.
- **The examples run in the tests,** so the documentation can't drift from
  the code.

## Non-goals

- **Management, read and auth routes.** That covers keys, users, orgs,
  apps, admin settings, logs, audit, the model catalog,
  `GET /v1/generations/{id}` and its assets, login and OAuth. A caller on
  custom auth polls through `GET /v1/videos/{id}` and
  `GET /v1/models3d/{id}`, which are hooked.
- **Webhooks.** The post hook does not see webhook payloads.
- **LiteLLM's other hooks.** That includes the moderation hook, the failure
  and streaming hooks, the response-headers hook, the model-listing filter and
  the logging callbacks.
- **Setting hooks from the dashboard.** They are YAML only for now. The
  planned settings work (storage, cache, access policy) may expose them
  later.
- **Hot reload.** Changing a hook takes a litegen restart, as with LiteLLM.
- **Per-route filters in the YAML.** A hook reads `request.path` and returns
  nothing for the routes it doesn't care about.
- **Running hook code inside the litegen process.** That rules out pyo3,
  V8 and QuickJS.

## Anti-goals (failure even if it works)

- **A crashing or slow `pre_call` must never let a request through by
  default.** For an auth hook, failing open is a security hole.
- **A hook must never be able to skip litegen's own checks.** Anything it
  changes is authenticated and validated afterwards like any other request.
- **Hooks must not slow litegen down when none are configured.** With no
  `hooks:`, the request path is exactly what it is today.

## Following LiteLLM (decided)

- **Hooks are trusted code, as in LiteLLM.** LiteLLM's `custom_auth` gets the
  whole `Request` and the raw `api_key` the caller sent. Its callbacks run
  inside the proxy process, with everything that process can reach. Its only
  redaction is in logs. litegen does the same:
  - a hook sees the raw request, including any litegen key or session cookie
    the caller sent; the dashboard Playground, for one, sends the master key
    on every call;
  - the hook process inherits litegen's environment, plus any `env:` the
    hook declares;
  - litegen never logs what a hook sees.

  The operator should treat a hook file like the litegen binary itself.
- **Refusing a request**, as LiteLLM's hooks do:
  - **Raise an error carrying a status.** In Python, an exception with
    `status_code` and `detail` (FastAPI's `HTTPException` shape, which
    LiteLLM uses). In Node, an `Error` with `status` or `statusCode`. The
    caller gets that status and message.
  - **Return a string.** That refuses with 400 and the string as the
    message, which is how LiteLLM answers a string returned for a
    non-chat endpoint.
  - The caller sees litegen's usual error body: `error.code`
    `refused_by_hook`, `error.message`, and any headers the error carries
    (`WWW-Authenticate`, `Retry-After`).
  - Any other exception is a hook failure (see "Failure policy").
- **Browsers, as LiteLLM's CORS works:**
  - A browser may send any request header (the preflight mirrors
    `Access-Control-Request-Headers`), so a customer's own auth header gets
    through. LiteLLM allows every request header too.
  - Allowed origins stay where they are: `cors.allowed_origins`, overridden
    by `LITEGEN_CORS_ORIGINS`, the counterpart of `LITELLM_CORS_ORIGINS`.
  - Credentials are allowed only with an explicit origin list, never with
    `*`, as LiteLLM switches them off under `*`.
  - The exposed response headers stay a fixed list, as LiteLLM's are.
- **No extra CSRF check.** LiteLLM has none. litegen's own CSRF check still
  guards its own session cookie. A hook that authenticates from a cookie of
  the customer's must check `Origin` (or `Sec-Fetch-Site`) itself. The
  documentation says so, and the sample cookie hook does it.

## How it is built (decided)

- **A long-lived child process per hook file.** litegen starts it once at
  boot, not once per request.
  - The process runs a small runner script that ships inside the litegen
    binary: one runner for Python, one for Node. The runner loads the
    operator's file and answers calls over the child's stdin and stdout.
  - The protocol is newline-delimited JSON. Each call carries an id, so
    calls overlap and answers can come back out of order. The Node runner is
    async throughout. The Python runner uses asyncio and runs sync hook
    functions in a thread pool.
  - The child's stderr goes to litegen's log, tagged with the hook's name.
- **Interpreter.** `python3`, `node` or `bun` from `PATH`, as `runtime:` or
  the extension says (see Goal 1), unless the hook sets `command:`.
- **Fail fast at boot.** litegen refuses to start if a hook file is missing,
  fails to load, or defines neither `pre_call` nor `post_call`. It also
  refuses if the `hooks:` section doesn't parse, even though `main.rs` falls
  back to the default config for other load errors today. That fallback is a
  separate gap, noted for later.
- **Restart on crash.** A child that exits is restarted with backoff. A call
  made while it is down takes the hook's failure policy.
- **Placement in the router.** The generation route group gets its own
  layer stack instead of sharing the global auth layer. From outside in:
  `pre_call`, then auth, then `post_call`, then the group's existing layers,
  then the handler. `post_call` sits inside auth so it can read the
  authenticated caller from the request.
- **Configuration**, for example:

  ```yaml
  hooks:
    - path: hooks/custom_auth.py       # relative to litegen.yaml; .py → python3
      timeout_ms: 2000                 # per call; default 2000
      on_error: refuse                 # refuse | allow — see failure policy
      env:                             # added to litegen's environment
        JWKS_URL: https://id.example.com/.well-known/jwks.json
    - path: hooks/tag_results.mjs      # .js/.mjs/.cjs → node
    - path: hooks/rewrite.ts           # .ts → bun
      runtime: bun                     # optional: python | node | bun
  ```

## What a hook sees

| Request | `pre_call` body | `post_call` response body |
|---|---|---|
| JSON (`application/json`) | the parsed JSON, which it may replace | the parsed JSON, which it may replace |
| multipart generation call (a `request` JSON part plus image files) | the `request` part's JSON, which it may change; the file parts pass through untouched, and the hook sees only their names, sizes and content types | JSON |
| raw bytes (the converter's mesh upload) | none: headers and query only | none: headers only; the converted file streams through untouched |
| `GET` status checks | none | JSON |

- Bodies are classified by route, as the validator does, not by
  `Content-Type`. The validator reads any non-multipart generation body as
  JSON, so the hook layer must agree with it about what the handler will
  see.
- Bodies follow the generation routes' existing 25 MiB limit.
- The request the hook returns is re-encoded: the JSON is re-serialized, or
  the multipart is rebuilt with the new `request` part. `Content-Length` is
  recomputed and hop-by-hop headers are dropped.

## Failure policy

- `pre_call` crash, timeout or malformed answer: the request is refused,
  503, error code `hook_failed`, naming the hook. `on_error: allow` passes
  the request on unchanged. That is only safe for a hook that doesn't
  decide access.
- `post_call` failure *(my call)*: litegen's own response goes out
  unchanged and the failure is logged. A generation is already done and
  billed by then, and refusing would cost the caller a paid result that
  they could only fetch again through routes the hook doesn't cover.
  `on_error: refuse` replaces it with the 503 instead. Use that for a hook
  that must strip or rewrite something before the caller sees it.
- A hook's own refusal is not a failure (see "Refusing a request").

## Calls I made (the operator didn't specify)

- **Process isolation over in-process embedding.** One child per file keeps
  Python and Node out of the litegen binary and lets a hook use any library.
- **Post failure lets the response through by default.** See "Failure
  policy" for the reasoning.
- **Names.** The hook functions are `pre_call` and `post_call`, and the YAML
  key is `hooks:`.
- **The dashboard Playground is not special-cased.** Its generation calls
  go through the hooks too. An auth hook should leave alone a request that
  already carries a litegen key, or refuse it if it wants to force its own
  auth. The documentation says so.

## Constraints

- **Keep the request path fast.** One extra JSON round trip over a pipe per
  hook per request. Aim for under about 1 ms of overhead on top of the
  hook's own work, for a 10 KB body.
- **Buffering under load.** `pre_call` buffers the body before litegen's
  backpressure slot is taken (the slot is acquired in the `Validated*`
  extractor today). Bound the hook layer's in-flight buffering so that a
  burst can't hold an unbounded number of 25 MiB bodies.
- **Python, Node and Bun in the image.** The release image
  (`debian:trixie-slim`) has none of them. A deployment with hooks has to
  provide the runtime it uses.
- **OpenAPI.** No new routes or wire shapes, apart from documenting the
  `hook_failed` and `refused_by_hook` error codes. Ping the peer session
  before changing `openapi.rs`.

## Left to the builder

- The exact JSON-lines message shapes, ids and framing, and the runners'
  internals.
- The restart backoff schedule. Whether `/health` reports each hook's
  process state.
- The bound on in-flight buffering in the hook layer.
- How large inline base64 images in a `post_call` body are handled.
- How the release image offers the runtimes: documentation, a build
  argument or a separate tag.
- How the tests are laid out across files, within the "Proof" section
  above.
