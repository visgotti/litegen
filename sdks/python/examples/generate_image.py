"""Example: generate an image with the LiteGen Python SDK."""
import os

from litegen import LiteGenClient


def main() -> None:
    client = LiteGenClient(
        base_url=os.environ.get("LITEGEN_BASE_URL", "http://localhost:4000"),
        api_key=os.environ.get("LITEGEN_API_KEY"),
    )
    result = client.images.generate(
        prompt="a serene mountain landscape at sunset, oil painting",
        model="openai/dall-e-3",
        size="1024x1024",
        quality="hd",
    )
    print("Image URL:", result["data"][0].get("url"))
    print("Provider:", result["provider"])
    if result.get("usage"):
        print("Cost USD:", result["usage"]["cost_usd"])


if __name__ == "__main__":
    main()
