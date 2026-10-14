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
- **Lax mode:** with `strict=False`, params the model doesn't support are dropped instead of rejected, and `job.dropped_params` lists them.
- **Cancel** an in-flight job with `client.generations.cancel(job.id)`.
- **Convert** a mesh you already have: `client.models3d.convert(data, from_format="glb", to_format="stl")` returns a dict with `content` (bytes), `content_type`, `filename`, `format` and `losses` (what the target container could not keep, e.g. `["uvs"]`). It is a standalone utility: nothing is generated or billed. `client.models3d.convert_formats()` lists the containers it handles and its size cap.
- **Timeout:** polling gives up after 30 minutes by default (`MODEL3D_DEFAULT_TIMEOUT`; override with `timeout=` in seconds) and raises `LiteGenPollingTimeoutError`. That stops the waiting, not the job, which can still complete and be billed: resume with `client.models3d.wait_for_completion(job_id)`.
- **Types:** responses are plain dicts. For static checking, import the TypedDicts from `litegen.types` (`Model3dGenerationResponse`, `Model3dAsset`, `Model3dGenerateParams`, ...). `client.models.list(media_type="model3d")` lists the 3D models.

The async client mirrors this: `await client.models3d.generate(...)`, `async for update in client.models3d.generate(...)`, `await job.id()` and `await job.dropped_params()`. See [`examples/generate_3d.py`](./examples/generate_3d.py).

## Links

- Website: https://litegen.ai
- License: MIT
