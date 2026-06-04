/**
 * Static, non-translatable site configuration. Translatable copy lives in
 * `messages/<locale>.json`; this holds URLs and the structural lists (features,
 * providers) that pair message keys with icons/metadata.
 */
export const siteConfig = {
  name: 'LiteGen',
  url: process.env.NEXT_PUBLIC_SITE_URL ?? 'https://litegen.dev',
  githubUrl: 'https://github.com/visgotti/litegen',
  docsUrl: 'https://github.com/visgotti/litegen#readme',
  licenseUrl: 'https://github.com/visgotti/litegen/blob/main/LICENSE',
} as const;

/** Feature cards: message key (under `features.items`) + lucide icon name. */
export const FEATURES = [
  { key: 'unified', icon: 'Plug' },
  { key: 'routing', icon: 'Route' },
  { key: 'caching', icon: 'Zap' },
  { key: 'cost', icon: 'LineChart' },
  { key: 'keys', icon: 'KeyRound' },
  { key: 'observability', icon: 'Activity' },
  { key: 'dashboard', icon: 'LayoutDashboard' },
  { key: 'selfhosted', icon: 'Server' },
] as const;

/**
 * Supported providers (brand names — not translated), each tagged with the
 * modalities LiteGen integrates for it. The Providers section renders these as
 * capability-tagged chips; the "Image"/"Video" tag words are translated.
 */
export interface ProviderInfo {
  name: string;
  image?: boolean;
  video?: boolean;
  /** Logo filename under public/logos/ (full-colour brand mark). */
  logo?: string;
}

export const PROVIDERS: readonly ProviderInfo[] = [
  // First-party APIs we integrate directly. Marquee names first.
  { name: 'OpenAI', image: true, video: true, logo: 'openai.png' },
  { name: 'Google', image: true, video: true, logo: 'google.png' },
  { name: 'Stability AI', image: true, logo: 'stability-ai.webp' },
  { name: 'Black Forest Labs', image: true, logo: 'black-forest-labs.png' },
  { name: 'Ideogram', image: true, logo: 'ideogram.svg' },
  { name: 'Recraft', image: true, logo: 'recraft.png' },
  { name: 'Leonardo.Ai', image: true, video: true, logo: 'leonardo-ai.png' },
  { name: 'Runway', image: true, video: true, logo: 'runway.png' },
  { name: 'Luma', image: true, video: true, logo: 'luma.png' },
  { name: 'Kling', image: true, video: true, logo: 'kling.png' },
  { name: 'MiniMax', image: true, video: true, logo: 'minimax.png' },
  { name: 'ByteDance', image: true, video: true, logo: 'bytedance.svg' },
  { name: 'Amazon Bedrock', image: true, video: true, logo: 'amazon-bedrock.png' },
  { name: 'Tencent Hunyuan', image: true, video: true, logo: 'tencent-hunyuan.svg' },
  { name: 'Vidu', video: true, logo: 'vidu.png' },
  { name: 'PixVerse', video: true, logo: 'pixverse.png' },
  // Aggregators / self-hosted.
  { name: 'Replicate', image: true, video: true, logo: 'replicate.png' },
  { name: 'Fal', image: true, video: true, logo: 'fal.png' },
] as const;

/** The quickstart snippet (code is not translated). */
export const QUICKSTART_CODE = `curl https://your-litegen-host/v1/images/generations \\
  -H "Authorization: Bearer $LITEGEN_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "openai/dall-e-3",
    "prompt": "a red panda coding at a desk, cinematic lighting",
    "size": "1024x1024"
  }'`;

/**
 * First-party SDK usage snippets (code is not translated). Mirrors the runnable
 * scripts in `sdks/typescript/examples` and `sdks/python/examples`.
 */
export const SDK_TS_INSTALL = `npm install @litegen/sdk`;

export const SDK_TS_CODE = `import { LiteGenClient } from '@litegen/sdk';

const client = new LiteGenClient({
  baseUrl: process.env.LITEGEN_BASE_URL ?? 'http://localhost:4000',
  apiKey: process.env.LITEGEN_API_KEY,
});

// Image — returns the result directly
const image = await client.images.generate({
  prompt: 'a red panda coding at a desk, cinematic lighting',
  model: 'openai/dall-e-3',
  size: '1024x1024',
});
console.log(image.data[0]?.url);

// Video — await resolves to the finished job (submits + polls under the hood)
const video = await client.videos.generate({
  prompt: 'a timelapse of clouds drifting over a city',
  model: 'runway/gen-3',
  duration_seconds: 5,
});
console.log(video.video_url);

// …or stream progress as it runs:
for await (const update of client.videos.generate({
  prompt: 'a timelapse of clouds drifting over a city',
  model: 'runway/gen-3',
})) {
  console.log(update.status, update.progress);
}`;

export const SDK_PY_INSTALL = `pip install litegen`;

export const SDK_PY_CODE = `from litegen import LiteGenClient

client = LiteGenClient(
    base_url="http://localhost:4000",
    api_key="lg-...",
)

# Image — returns the result directly
image = client.images.generate(
    prompt="a red panda coding at a desk, cinematic lighting",
    model="openai/dall-e-3",
    size="1024x1024",
)
print(image["data"][0]["url"])

# Video — .result() blocks until the job finishes (submits + polls under the hood)
video = client.videos.generate(
    prompt="a timelapse of clouds drifting over a city",
    model="runway/gen-3",
    duration_seconds=5,
).result()
print(video["video_url"])

# …or stream progress as it runs:
for update in client.videos.generate(
    prompt="a timelapse of clouds drifting over a city",
    model="runway/gen-3",
):
    print(update["status"], update["progress"])`;
