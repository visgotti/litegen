/**
 * Static, non-translatable site configuration. Translatable copy lives in
 * `messages/<locale>.json`; this holds URLs and the structural lists (features,
 * providers) that pair message keys with icons/metadata.
 */
import { PROVIDER_CAPABILITIES } from '@/components/infra-flow/capabilities.generated';
import providerBrands from './providers.json';

export const siteConfig = {
  name: 'LiteGen',
  url: process.env.NEXT_PUBLIC_SITE_URL ?? 'https://litegen.ai',
  /** Hosted dashboard / app (OAuth sign-in). The "Dashboard" CTAs point here. */
  appUrl: 'https://app.litegen.ai',
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
 * Supported providers: every provider in litegen's catalog, in display order.
 * The brand data (name, logo) lives in `providers.json`; what each one makes
 * comes from the catalog-derived capability snapshot, so a chip's Image /
 * Video / 3D tags can't drift from what ships. `derive-capabilities.test.mjs`
 * fails when the list and the catalog disagree.
 */
export interface ProviderInfo {
  /** The catalog's provider id, e.g. "bfl". */
  id: string;
  name: string;
  /** Logo filename under public/logos/ (full-colour brand mark). */
  logo?: string;
  image: boolean;
  video: boolean;
  model3d: boolean;
}

export const PROVIDERS: readonly ProviderInfo[] = providerBrands.map((p) => {
  const outputs = PROVIDER_CAPABILITIES[p.id]?.outputs ?? { image: false, video: false, model3d: false };
  return { ...p, ...outputs };
});

/** The quickstart snippet (code is not translated). */
export const QUICKSTART_CODE = `curl https://your-litegen-host/v1/images/generations \\
  -H "Authorization: Bearer $LITEGEN_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{
    "model": "openai/gpt-image-2",
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
  model: 'openai/gpt-image-2',
  size: '1024x1024',
});
console.log(image.data[0]?.url);

// Video — await resolves to the finished job (submits + polls under the hood)
const video = await client.videos.generate({
  prompt: 'a timelapse of clouds drifting over a city',
  model: 'runway/gen4.5',
  duration_seconds: 5,
});
console.log(video.video_url);

// …or stream progress as it runs:
for await (const update of client.videos.generate({
  prompt: 'a timelapse of clouds drifting over a city',
  model: 'runway/gen4.5',
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
    model="openai/gpt-image-2",
    size="1024x1024",
)
print(image["data"][0]["url"])

# Video — .result() blocks until the job finishes (submits + polls under the hood)
video = client.videos.generate(
    prompt="a timelapse of clouds drifting over a city",
    model="runway/gen4.5",
    duration_seconds=5,
).result()
print(video["video_url"])

# …or stream progress as it runs:
for update in client.videos.generate(
    prompt="a timelapse of clouds drifting over a city",
    model="runway/gen4.5",
):
    print(update["status"], update["progress"])`;
