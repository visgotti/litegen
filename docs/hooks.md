# Generation call hooks

Hooks let you run your own code around litegen's AI generation calls. A hook
is a **Python, Node or Bun file** (Bun runs TypeScript as it is) named in
`litegen.yaml`. It can:

- **authenticate callers your own way** before litegen does: read your token,
  cookie or header, then inject a litegen API key;
- **change a request** before litegen validates and runs it: default a
  param, alias a model;
- **refuse a request**, with your own status, message and headers;
- **change a response** before it goes back: tag it, add fields or headers;
- **decide who opens a stored file** (`file_access`): keep each of your
  customers to their own images, videos and meshes.

litegen's own authentication, authorization and validation still run on
whatever a hook passes on. A hook can stand in for a caller's credentials,
but it can't get anything past litegen's checks.

## Compared with LiteLLM

The design follows LiteLLM's proxy hooks (`custom_auth`,
`async_pre_call_hook`, `async_post_call_success_hook`): the same trust model,
the same ways to refuse, and the same "custom auth first, then litegen's own
auth" fallback as LiteLLM's `custom_auth_settings: mode: auto`. Beyond that:

- **Three runtimes, not one.** Python, Node or Bun, and Bun runs TypeScript
  directly. LiteLLM takes Python only.
- **Mix languages in one chain.** A Python auth hook can run first, then a
  Bun rewrite.
- **A hook can't take the gateway down.** Each hook runs in its own process.
  A crash, a hang or a memory blow-up stays there: litegen refuses (or, if
  you choose, passes) the calls it affects and restarts the hook.
- **A time limit and a failure policy per hook.**
- **Custom auth and request rewriting in one function.** `pre_call` runs
  before authentication.
- **The finished result of async jobs.** For video and 3D, `post_call` sees
  the status answer that carries the finished assets.
- **Sync or async functions** in every language.

## Configuration

```yaml
hooks:
  - path: hooks/custom_auth.py     # .py → python3
    timeout_ms: 2000               # per call (default 2000)
    on_error: refuse               # refuse | allow (default: see below)
    env:                           # added to the environment the hook inherits
      ACME_TOKENS: "alice-token:alice"
  - path: hooks/rewrite.ts         # .ts → bun
  - path: hooks/tag_results.mjs    # .js/.mjs/.cjs → node
    runtime: bun                   # optional: python | node | bun
    command: /usr/local/bin/bun    # optional: the executable to run it with
```

| key | meaning | default |
|---|---|---|
| `path` | The hook file, relative to the folder of the config file that declares it, or absolute. For a `litegen.yaml` found in the directory litegen runs in, that is that directory; for a file named with `--config`, it is the file's own folder, as LiteLLM's custom callbacks do ([configuration.md](configuration.md#hook-paths)). | required |
| `runtime` | `python`, `node` or `bun`. | from the extension (table below) |
| `command` | The executable that runs the hook, for a virtualenv's Python or a pinned Node or Bun. | `python3`, `node` or `bun` |
| `timeout_ms` | How long one call may take. | 2000 |
| `on_error` | What a failure does, for every hook of the file: `refuse` answers 503, `allow` carries on as if the hook returned nothing. | `pre_call` and `file_access`: `refuse`; `post_call`: `allow` |
| `env` | Extra environment variables for the hook. | none |

| extension | default runtime | also allowed |
|---|---|---|
| `.py` | `python` | — |
| `.js`, `.mjs`, `.cjs` | `node` | `bun` |
| `.ts`, `.mts` | `bun` | `node` (Node 22.18+ strips types itself) |

- **Order.** Hooks run in the order listed. `pre_call`s each see the
  previous one's output, and the first refusal stops the chain. `post_call`s
  run in the same order.
- **A file defines any of `pre_call`, `post_call` and `file_access`.**
- **Startup refuses mistakes.** litegen won't start if a hook file is
  missing, fails to load, or defines none of the three, or if an entry has an
  unknown key. If `litegen.yaml` declares hooks but doesn't load, it won't
  fall back to its defaults (which would drop the hooks); it stops.
- **Restarting.** Changing a hook takes a restart of litegen, as with
  LiteLLM.

## Which routes are hooked

Exactly the routes an API key needs the `generate` scope for:

| route | `pre_call` sees the body? | `post_call` sees the body? |
|---|---|---|
| `POST /v1/images/generations`, `POST /v1/images/segmentations` | yes (JSON, or the multipart `request` part) | yes |
| `POST /v1/videos/generations`, `POST /v1/models3d/generations` | yes | yes |
| `POST /v1/images/cost`, `/v1/videos/cost`, `/v1/models3d/cost` | yes | yes |
| `GET /v1/videos/{id}`, `GET /v1/models3d/{id}` (status) | no body | yes |
| `POST /v1/utils/models3d/convert` | no: the upload streams through unread | no: the converted file streams through; headers only |
| `GET /v1/generations/{id}/assets/{index}` (a stored file; also runs `file_access`) | no body | no: the file streams through; headers only |

The file route is hooked so custom auth reaches a stored file, as it does a
generation ([docs/storage-access.md](storage-access.md)). Every other
management, read and auth route never reaches a hook: keys, users, orgs,
admin settings, logs, `GET /v1/generations/{id}`, login and the like.
A caller on custom auth polls a job through `GET /v1/videos/{id}` or
`GET /v1/models3d/{id}`, which are hooked. Webhooks don't pass through hooks.

## What a hook gets

`pre_call(request)` runs **before authentication**.
`post_call(request, response, caller)` runs **after the handler**, inside
authentication. It sees every response the handler sends, validation
errors included, so check `response.status`. Auth refusals never reach it,
and neither does the answer to a client that hung up first: a billed
request (a generation or segmentation) still runs to its end then, and is
charged, logged and delivered to the key's webhook, but its response has no
one to go to and `post_call` doesn't run for it.

**`request`**

| field | |
|---|---|
| `method` | `"POST"`, `"GET"` |
| `path` | `"/v1/images/generations"` |
| `query` | the raw query string, `""` if none |
| `headers` | lowercase names to values; a repeated header's values are joined with `", "`; a value that isn't UTF-8 is left out (and kept on the request) |
| `body` | the JSON body, or a multipart call's `request` part; `null` where the body isn't shown |
| `files` | a multipart call's other parts: `{name, filename, content_type, size}` (never their bytes) |

**`response`** (in `post_call`): `{status, headers, body}`. `body` is the
JSON litegen answered, or `null` for anything else.

`file_access(request, file, caller)` runs on litegen's file route, **after
litegen's own owner check** passed (see
[docs/storage-access.md](storage-access.md)). It can only refuse: a file the
owner check refuses never reaches it.

It also runs when a request names a stored generation as an input:
`reference_images`, `source_video` or `source_model` of type `generation`.
litegen first picks the file the job will read (the image, the clip, or the
mesh in the container the model takes), then asks `file_access` about that
file, before reading it or reserving anything. `request` is then that request
(`method` `POST`, its path and headers as `pre_call` left them, `body`
`null`), and a refusal answers the generation request as it would the file
route. So a customer can't chain another customer's file into their own job.

**`file`** (in `file_access`): the stored file and who made it.

| field | |
|---|---|
| `generation_id`, `index` | the file the route names, or the one a request reads as its input |
| `kind`, `format`, `size_bytes` | `image`, `video`, `mesh`, `texture`…; `png`, `mp4`, `glb`…; its size |
| `media_type`, `model`, `created_at` | of the generation that made it |
| `created_by` | `{key_id, app_id, org_id, end_user}`: the key, app and org that made it, and the end user it was made for (`x-litegen-end-user` on the request that made it) |

**`caller`** (in `post_call` and `file_access`): who litegen authenticated.

| field | |
|---|---|
| `key_id` | the API key's id, or `null` (the master key, a session) |
| `org_id`, `app_id` | the key's org and app, if any |
| `end_user` | the `x-litegen-end-user` header |
| `user` | `{id, email, role}` for a signed-in session, else `null` |

## What a hook answers

| a hook… | means |
|---|---|
| returns nothing (`None`, `undefined`) | carry on unchanged |
| returns its argument, changed | `pre_call`: litegen goes on with the changed `headers`, `query` and `body` (a changed `method` or `path` is an error); `post_call`: litegen sends the changed `status`, `headers` and `body` (a non-JSON body can't change) |
| returns a string | refuse with **400** and that message (as LiteLLM does for a non-chat endpoint); from `file_access`, **403** |
| `file_access` returns `true` or `false` | serve the file, or refuse with **403** |
| raises an error carrying a status (400–599) | refuse with that status and message, plus any `headers` it carries. In Python, an exception with `status_code`, `detail` and optionally `headers`, the shape of FastAPI's `HTTPException`; in Node and Bun, an `Error` with `status` or `statusCode` and optionally `headers`. |
| raises anything else, times out, crashes, or answers something litegen can't use | a **failure**: `on_error` decides |

A body a `pre_call` changed is validated as the caller's would be. A field
it adds that the request has no such name for is refused with **400
`param_unsupported`**, or dropped and named in `X-Litegen-Dropped-Params`
when the body has `strict: false` (see "Strict requests" in the README).
Pass data meant only for your own hooks in a header instead.

A refusal reaches the caller as:

```json
{ "error": { "type": "hook_error", "code": "refused_by_hook", "message": "send a valid x-acme-token", "hook": "custom_auth.py" } }
```

A failure that refuses is a **503** with `"code": "hook_failed"` and the
reason in `message`. A request with a body counts against
`backpressure.max_in_flight` once its body has arrived: with that many
already counted, a hooked route answers **503 `server_at_capacity`** with
`Retry-After: 1` at once, before reading the body, as litegen's own
backpressure does. A request still sending its body holds only
the bytes it has sent, so a slow or stalled client blocks no one, and a body
that takes longer than `server.request_body_timeout_secs` (default 300 s) is
answered **408 `body_timeout`**. Requests with no body (status reads, files)
never wait on that count.

## Examples

Every example below is in `examples/hooks/`, and litegen's end-to-end tests
run each one against the real server.

### Custom auth, in Python

Callers send their own token; the hook injects your litegen key and names
the end user, so per-user budgets and spend apply.

<!-- example: examples/hooks/custom_auth.py -->
```python
"""Custom auth for litegen: callers send your token, the hook injects a litegen key.

Callers send `x-acme-token: <token>` instead of a litegen API key. ACME_TOKENS
maps tokens to users (`token:user,token:user`). A known token gets litegen's
key (LITEGEN_APP_KEY) and is recorded as that end user, so per-user budgets
and spend apply. A request that already carries a litegen key is left to
litegen's own auth.

    hooks:
      - path: hooks/custom_auth.py
"""
import os


class Unauthorized(Exception):
    """Refuses the request: FastAPI's HTTPException shape, as LiteLLM uses."""

    status_code = 401

    def __init__(self, detail):
        super().__init__(detail)
        self.detail = detail
        self.headers = {"www-authenticate": "AcmeToken"}


TOKENS = dict(
    pair.strip().split(":", 1)
    for pair in os.environ.get("ACME_TOKENS", "").split(",")
    if ":" in pair
)


def pre_call(request):
    headers = request["headers"]
    if "authorization" in headers:
        return None  # already a litegen caller
    user = TOKENS.get(headers.get("x-acme-token", ""))
    if user is None:
        raise Unauthorized("send a valid x-acme-token")
    headers["authorization"] = "Bearer " + os.environ["LITEGEN_APP_KEY"]
    headers["x-litegen-end-user"] = user
    del headers["x-acme-token"]  # litegen doesn't need it
    return request
```

### The same, in Node or Bun

<!-- example: examples/hooks/custom_auth.mjs -->
```js
// Custom auth for litegen, in Node or Bun: callers send your token, the hook
// injects a litegen key. The same contract as custom_auth.py:
//   - `x-acme-token: <token>` instead of a litegen API key;
//   - ACME_TOKENS maps tokens to users (`token:user,token:user`);
//   - a known token gets LITEGEN_APP_KEY and is recorded as that end user;
//   - a request that already carries a litegen key is left to litegen.
//
//   hooks:
//     - path: hooks/custom_auth.mjs
const TOKENS = new Map(
  (process.env.ACME_TOKENS ?? "")
    .split(",")
    .filter((pair) => pair.includes(":"))
    .map((pair) => {
      const [token, ...user] = pair.trim().split(":");
      return [token, user.join(":")];
    }),
);

export function pre_call(request) {
  const { headers } = request;
  if (headers.authorization) return undefined; // already a litegen caller
  const user = TOKENS.get(headers["x-acme-token"] ?? "");
  if (!user) {
    // An error with a status refuses the request with that status.
    const refusal = new Error("send a valid x-acme-token");
    refusal.status = 401;
    refusal.headers = { "www-authenticate": "AcmeToken" };
    throw refusal;
  }
  headers.authorization = `Bearer ${process.env.LITEGEN_APP_KEY}`;
  headers["x-litegen-end-user"] = user;
  delete headers["x-acme-token"]; // litegen doesn't need it
  return request;
}
```

### A request rewrite, in Bun TypeScript

<!-- example: examples/hooks/rewrite.ts -->
```ts
// A request rewrite for litegen, in TypeScript: Bun runs it as it is.
// Callers may use short model aliases; litegen then validates the real model
// like any other request.
//
//   hooks:
//     - path: hooks/rewrite.ts
interface HookRequest {
  method: string;
  path: string;
  query: string;
  headers: Record<string, string>;
  body: Record<string, unknown> | null;
  files: Array<{ name: string; filename?: string; content_type?: string; size: number }>;
}

const ALIASES: Record<string, string> = {
  "fast-image": "mock/visual-image-gen",
};

export function pre_call(request: HookRequest): HookRequest | undefined {
  const model = request.body?.model;
  if (typeof model !== "string" || !(model in ALIASES)) return undefined;
  request.body!.model = ALIASES[model];
  return request;
}
```

### Tag every answer (`post_call`), in Node or Bun

<!-- example: examples/hooks/tag_results.mjs -->
```js
// A post_call hook for litegen, in Node or Bun: tag every successful
// generation answer before it goes back. For video and 3D it sees every
// status answer, including the one that carries the finished result.
//
//   hooks:
//     - path: hooks/tag_results.mjs
export function post_call(request, response, caller) {
  if (response.status < 200 || response.status >= 300) return undefined;
  // A JSON answer can be changed; a file (the converter's) keeps its bytes.
  if (response.body && typeof response.body === "object") {
    response.body.served_by = "litegen+hooks";
  }
  response.headers["x-served-by"] = "litegen+hooks";
  return response;
}
```

### Each customer opens only their own files (`file_access`), in Python

With `storage_access: mode: private`, every file litegen hands out is its file
route, and custom auth (above) names each request's end user. litegen records
that end user on the generation; this hook compares it with whoever is asking:

<!-- example: examples/hooks/file_access_end_user.py -->
```python
"""Each of your customers opens only the files made for them.

litegen records who a generation was for (`x-litegen-end-user`, set here by
custom_auth.py) and passes it to `file_access` as file["created_by"]["end_user"];
whoever is asking now is caller["end_user"]. The hook runs on litegen's file
route, after litegen's own owner check, so it can only refuse. A file made for
no one in particular (no end user recorded) is left to that owner check.

    storage_access:
      mode: private
    hooks:
      - path: hooks/custom_auth.py
      - path: hooks/file_access_end_user.py
"""


def file_access(request, file, caller):
    made_for = file["created_by"].get("end_user")
    if made_for is not None and made_for != caller.get("end_user"):
        return False  # 403
    return None
```

### The same, in Node or Bun

<!-- example: examples/hooks/file_access_end_user.mjs -->
```js
// Each of your customers opens only the files made for them.
//
// litegen records who a generation was for (`x-litegen-end-user`, set here by
// custom_auth.mjs) and passes it to `file_access` as file.created_by.end_user;
// whoever is asking now is caller.end_user. The hook runs on litegen's file
// route, after litegen's own owner check, so it can only refuse. A file made
// for no one in particular (no end user recorded) is left to that owner check.
//
//     storage_access:
//       mode: private
//     hooks:
//       - path: hooks/custom_auth.mjs
//       - path: hooks/file_access_end_user.mjs

export function file_access(request, file, caller) {
  const madeFor = file.created_by.end_user;
  if (madeFor != null && madeFor !== caller.end_user) return false; // 403
  return undefined;
}
```

Alice opens her file; Bob, with his own token, gets the hook's 403:

```sh
curl http://localhost:4000/v1/generations/$ID/assets/0 -H 'x-acme-token: alice-token' -o fox.png
curl -i http://localhost:4000/v1/generations/$ID/assets/0 -H 'x-acme-token: bob-token'   # 403 refused_by_hook
```

### Cookie auth for browsers, with the CSRF check, in Python

<!-- example: examples/hooks/cookie_auth.py -->
```python
"""Cookie auth for litegen, for browsers: your session cookie in, a litegen key injected.

A browser on your site sends its `acme_session` cookie straight to litegen.
A cookie travels with cross-site requests too, so before trusting it the hook
checks `Origin` against ALLOWED_ORIGINS. litegen leaves that CSRF check to a
hook that authenticates from a cookie. ACME_TOKENS maps session tokens to
users (`token:user,token:user`).

    hooks:
      - path: hooks/cookie_auth.py
"""
import os
from http.cookies import SimpleCookie

ALLOWED_ORIGINS = {o.strip() for o in os.environ.get("ALLOWED_ORIGINS", "").split(",") if o.strip()}
TOKENS = dict(
    pair.strip().split(":", 1)
    for pair in os.environ.get("ACME_TOKENS", "").split(",")
    if ":" in pair
)


class Refused(Exception):
    """Refuses the request with `status_code` (FastAPI's HTTPException shape)."""

    def __init__(self, status_code, detail):
        super().__init__(detail)
        self.status_code = status_code
        self.detail = detail


def pre_call(request):
    headers = request["headers"]
    if "authorization" in headers:
        return None  # already a litegen caller
    origin = headers.get("origin")
    if origin and origin not in ALLOWED_ORIGINS:
        raise Refused(403, f"cross-site request from {origin} refused")
    session = SimpleCookie(headers.get("cookie", "")).get("acme_session")
    user = TOKENS.get(session.value) if session else None
    if user is None:
        raise Refused(401, "sign in first")
    headers["authorization"] = "Bearer " + os.environ["LITEGEN_APP_KEY"]
    headers["x-litegen-end-user"] = user
    return request
```

Chained, the first three make a working gateway:

```yaml
hooks:
  - path: hooks/custom_auth.py
  - path: hooks/rewrite.ts
  - path: hooks/tag_results.mjs
```

```sh
curl -X POST http://localhost:4000/v1/images/generations \
  -H 'x-acme-token: alice-token' -H 'content-type: application/json' \
  -d '{"model": "fast-image", "prompt": "a fox"}'
```

## Writing hooks

- **Return the object you were given, changed.** It's simplest to edit
  `request` or `response` in place and return it.
- **Print freely.** Anything a hook writes to stdout goes to litegen's log,
  not to the protocol.
- **Python.** `async def` hooks run on the runner's event loop; plain `def`
  hooks run in a thread pool, so blocking calls are fine. The hook file's
  directory is on `sys.path`, so it can import its neighbours.
- **Node and Bun.** Hooks may be `async`. Don't block the event loop: every
  call to that hook waits behind it.
- **Leave a litegen caller alone.** Return nothing when a request already
  carries a litegen key, unless your hook means to replace litegen's auth.
  The dashboard's Playground sends the master key on every generation call.
- **Read configuration from the environment.** A hook inherits litegen's
  environment, plus its own `env:`.

## The trust model

A hook is trusted code, as LiteLLM's callbacks are. It sees the raw request,
including any litegen API key or session cookie a caller sends, and it
inherits litegen's environment, which can include your provider keys. Treat a
hook file like the litegen binary itself: review it, and keep it where only
you can change it. litegen never logs what a hook sees.

## Browsers: CORS and CSRF

- **Any request header passes CORS,** as in LiteLLM, so a browser can send
  your custom auth header.
- **Origins** come from `cors.allowed_origins` or `LITEGEN_CORS_ORIGINS`.
- **Credentials** (`cors.allow_credentials`) are only allowed with an
  explicit origin list, never with `*`.
- **CSRF is your hook's duty if it trusts a cookie.** A cookie travels with
  cross-site requests too, and litegen's own CSRF check covers only
  litegen's session cookie, as LiteLLM has none. Check `origin` (or
  `sec-fetch-site`) before trusting the cookie, as `cookie_auth.py` above
  does.

## Deploying

The release image (`debian:trixie-slim`) carries no Python, Node or Bun.
Add the runtime your hooks use, for example:

```dockerfile
FROM litegen:latest
USER root
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
USER 1000:1000
COPY hooks/ /app/hooks/
```

Each hook runs as one long-lived process beside litegen. It is started at
boot, restarted with backoff if it dies, and stopped with litegen; it exits
on its own if litegen is killed.
