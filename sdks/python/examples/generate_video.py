"""Example: generate a video and wait for completion."""
import os

from litegen import LiteGenClient


def main() -> None:
    client = LiteGenClient(
        base_url=os.environ.get("LITEGEN_BASE_URL", "http://localhost:4000"),
        api_key=os.environ.get("LITEGEN_API_KEY"),
    )

    # `generate` returns a handle. Call `.result()` to block until the video is
    # finished — it submits and polls under the hood:
    final = client.videos.generate(
        prompt="a timelapse of clouds drifting over a quiet city",
        model="runway/gen-3",
        duration_seconds=5,
        interval=5.0,
        timeout=600.0,
    ).result()

    if final["status"] == "completed":
        print("Video URL:", final.get("video_url"))
    else:
        print("Generation failed:", final.get("error"))

    # …or stream progress as it runs:
    #
    #   for update in client.videos.generate(prompt="...", model="..."):
    #       print(f"{update['status']} — {update['progress']}%")


if __name__ == "__main__":
    main()
