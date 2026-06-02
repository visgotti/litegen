# litegen — Python SDK

First-party Python SDK for [LiteGen](https://github.com/litegen/litegen).

```python
from litegen import LiteGenClient

client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
result = client.images.generate(
    prompt="a serene mountain landscape at sunset",
    model="openai/dall-e-3",
    size="1024x1024",
    quality="hd",
)
print(result["data"][0]["url"])
```

See [`examples/`](./examples) for runnable scripts.
