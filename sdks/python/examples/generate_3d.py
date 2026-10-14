"""Example: generate a 3D model and wait for completion."""
import os

from litegen import LiteGenClient


def main() -> None:
    client = LiteGenClient(
        base_url=os.environ.get("LITEGEN_BASE_URL", "http://localhost:4000"),
        api_key=os.environ.get("LITEGEN_API_KEY"),
    )

    # `generate` returns a handle. Call `.result()` to block until the job is
    # finished — it submits and polls under the hood, for up to 30 minutes by
    # default. Every format in `output_formats` is present on a completed job,
    # or the job fails.
    final = client.models3d.generate(
        model="fal/hyper3d-rodin",
        prompt="a weathered bronze garden gnome holding a lantern",
        output_formats=["glb", "obj"],
        interval=5.0,
    ).result()

    if final["status"] == "completed":
        for asset in final.get("assets", []):
            print(f"{asset['kind']:<8} {asset['format']:<5} {asset['url']}")
    else:
        print("Generation", final["status"], "-", final.get("error"))

    # Image-to-3D: image-only models take no prompt, just an `init` image:
    #
    #   client.models3d.generate(
    #       model="fal/trellis",
    #       reference_images=[{"type": "url", "value": "https://...", "role": "init"}],
    #   ).result()
    #
    # …or stream progress as it runs:
    #
    #   for update in client.models3d.generate(model="...", prompt="..."):
    #       print(f"{update['status']} — {update['progress']}%")


if __name__ == "__main__":
    main()
