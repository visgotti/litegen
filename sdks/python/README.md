# litegen-sdk — Python SDK for LiteGen

First-party Python SDK for **[LiteGen](https://litegen.ai)** — one API for every AI image, video & 3D generation model.

## Install

```bash
pip install litegen-sdk
```

The distribution is `litegen-sdk`; the import name is `litegen`:

```python
from litegen import LiteGenClient

client = LiteGenClient(
    base_url="https://app.litegen.ai/api",  # your LiteGen instance
    api_key="sk_live_...",                   # a LiteGen secret key
)

result = client.images.generate(
    prompt="a serene mountain landscape at sunset",
    model="openai/gpt-image-2",
    size="1024x1024",
)
print(result)
```

An async client (`from litegen import AsyncLiteGenClient`) and video and 3D generation with polling are also available. See [`examples/`](./examples) for runnable scripts.

## 3D models

3D generation is always asynchronous. `generate` returns a handle: block on it with `.result()`, or iterate it to stream progress.

```python
final = client.models3d.generate(
    model="fal/hyper3d-rodin",
    prompt="a weathered bronze garden gnome",
    output_formats=["glb", "obj"],
).result()  # polls until completed, failed or cancelled; a failure is returned, not raised

if final["status"] == "completed":
    for asset in final.get("assets", []):
        if asset["kind"] == "mesh":
            print(asset["format"], asset["url"])
else:
    print(final["status"], final.get("error"))

# Stream instead of blocking; `progress` is LiteGen's unified 0-100 value.
for update in client.models3d.generate(model="fal/hyper3d-rodin", prompt="a teapot"):
    print(update["status"], update["progress"])
```

- **Image-to-3D:** pass the picture as a reference image with the `init` role. Image-only models such as `fal/trellis` and `fal/hunyuan3d-v2` take no prompt, so leave it out: `client.models3d.generate(model="fal/trellis", reference_images=[{"type": "url", "value": "https://example.com/chair.png", "role": "init"}])`. Multiview models take one image per `view-front` / `view-back` / `view-left` / `view-right` / `view-top` role.
- **`output_formats` is a promise:** a `completed` job carries a mesh asset in every format you listed, or the job fails. The server decides which formats a model can deliver (see its `output_formats` param in `client.models.get(model_id)`). Any conversion happens on the server, never in the SDK.
- **Lax mode:** with `strict=False`, what strict mode refuses (params the model doesn't take, and fields the request has no such name for, one inside an object by its path, such as `animations[0].duration`) is dropped instead, and `job.dropped_params` lists it: at most 32 names, then a last `+N more` counting the rest.
- **What a model accepts:** `client.models.get(model_id)` returns its `params` and, beside them, `litegen_params`: the fields litegen applies itself (`ground`, `center`, `height_m`), with their bounds.
- **Cancel** an in-flight job with `client.generations.cancel(job.id)`.
- **Convert** a mesh you already have: `client.models3d.convert(data, from_format="glb", to_format="stl")` returns a dict with `content` (bytes), `content_type`, `filename`, `format` and `losses` (what the target container could not keep, e.g. `["uvs"]`). It is a standalone utility: nothing is generated or billed. `client.models3d.convert_formats()` lists the containers it handles and its size cap. Pass `package="zip"` to get a zip holding the file and whatever goes beside it (an OBJ's `.mtl` and textures); the SDK returns it unopened.
- **Timeout:** polling gives up after `JOB_DEFAULT_TIMEOUT` (2 hours 5 minutes) by default, past the 2 hours after which the server fails a job itself (override with `timeout=` in seconds), and raises `LiteGenPollingTimeoutError`. That stops the waiting, not the job, which can still complete and be billed: resume with `client.models3d.wait_for_completion(job_id)`.
- **Types:** responses are plain dicts. For static checking, import the TypedDicts from `litegen.types` (`Model3dGenerationResponse`, `Model3dAsset`, `Model3dGenerateParams`, ...). `client.models.list(media_type="model3d")` lists the 3D models.

The async client mirrors this: `await client.models3d.generate(...)`, `async for update in client.models3d.generate(...)`, `await job.id()` and `await job.dropped_params()`. See [`examples/generate_3d.py`](./examples/generate_3d.py).

## Timeouts, headers and metadata

- **Timeouts:** a request waits up to `DEFAULT_TIMEOUT` (690 s) unless `timeout=` says otherwise: a synchronous call can take three vendor attempts of up to 120 s each, after fetching a 3D operation's URL source for up to 300 s.
- **Response headers:** `on_response=` is called with every `httpx.Response`, errors included; `response_meta(response.headers)` reads litegen's (`dropped_params`, `rate_limit`, `idempotent_replayed`, `retry_after`). A `LiteGenAPIError` carries its response's `headers`. Quotes (`estimate_cost`), `images.segment` and image generations name what lax mode dropped there, in the same header.
- **Errors:** a non-2xx answer raises `LiteGenAPIError` (`LiteGenValidationError` for a `validation_error`) with its `status`, `code`, `retryable` and `retry_after`. `retryable` is true for a vendor's 5xx, 429 or timeout and for a 503 `circuit_open` (every deployment of a model route cooling down; `retry_after` says when the first is back), and false for a refusal. A 503 `server_at_capacity` also sets `retry_after`. A status poll waits out `Retry-After` itself; a submit is never sent twice.
- **Idempotency:** `idempotency_key=` on every billed submit replays the first answer to a repeated request, billed once.
- **Cancel** returns the row as settled (a cancelled job is refunded) with `vendor_cancelled`, whether the vendor took the cancel.
- **Metadata:** send `metadata=` (a string map, at most 1 KB) with any generation; it is echoed on every answer, and `logs.list(metadata=...)` / `stats.get(metadata=...)` split logs and spend by it.
- **Health:** `client.health.live()` and `client.health.ready()` are the liveness and readiness probes (no credential needed); `ready()` answers `{"status", "checks": {"db", "providers"}}` and raises `LiteGenAPIError` with `status` 503 while the server is not ready. `client.health.check()` checks the providers set up with the gateway's own keys, so it takes the master key (or, single-tenant, the `read` scope in the deployment's org).

## Versions

The package follows semantic versioning, and [CHANGELOG.md](./CHANGELOG.md) lists every release; `litegen.__version__` is the version you have.

## Links

- Website: https://litegen.ai
- License: MIT
