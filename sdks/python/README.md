# litegen-sdk — Python SDK for LiteGen

First-party Python SDK for **[LiteGen](https://litegen.ai)** — one API for every AI image & video generation model.

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
    model="openai/dall-e-3",
    size="1024x1024",
)
print(result)
```

An async client (`from litegen import AsyncLiteGenClient`) and video generation with polling are also available. See [`examples/`](./examples) for runnable scripts.

## Links

- Website: https://litegen.ai
- License: MIT
