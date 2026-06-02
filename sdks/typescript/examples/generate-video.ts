import { LiteGenClient, GenerationStatus } from "../src";

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? "http://localhost:4000",
  apiKey: process.env.LITEGEN_API_KEY,
});

// `generate` returns a handle that is awaitable (resolves to the finished job)
// and async-iterable (streams progress). Await it to block until the video is
// done — it submits and polls under the hood:
const video = await client.videos.generate(
  {
    prompt: "a timelapse of clouds drifting over a quiet city",
    model: "runway/gen-3",
    duration_seconds: 5,
  },
  { intervalMs: 5_000, timeoutMs: 10 * 60_000 },
);

if (video.status === GenerationStatus.Completed) {
  console.log("Video URL:", video.video_url);
} else {
  console.error("Generation failed:", video.error);
}

// …or stream progress as it runs:
//
//   for await (const update of client.videos.generate({ ... })) {
//     console.log(`${update.status} — ${update.progress}%`);
//   }
