# @litegen/sdk

First-party TypeScript/JavaScript SDK for **[LiteGen](https://litegen.ai)** — one API for every AI image & video generation model. Bring your own provider keys, or use a hosted LiteGen instance, and call dozens of image/video models through a single typed client.

## Install

```bash
npm install @litegen/sdk
```

## Quick start

```ts
import { LiteGenClient } from '@litegen/sdk';

const litegen = new LiteGenClient({
  baseUrl: 'https://app.litegen.ai/api', // your LiteGen instance
  apiKey: process.env.LITEGEN_API_KEY,   // a sk_live_… secret key
});

// Generate an image
const image = await litegen.images.generate({
  model: 'fal/flux/schnell',
  prompt: 'a red panda coding at night, cinematic lighting',
});
console.log(image);

// Generate a video — await the final result…
const video = await litegen.videos.generate({
  model: 'replicate/kling-v1',
  prompt: 'a timelapse of a city skyline at dusk',
});

// …or stream progress updates as they arrive
for await (const update of litegen.videos.generate({ model: 'replicate/kling-v1', prompt: '…' })) {
  console.log(update.status);
}
```

The client is fully typed (ESM + CJS builds, bundled `.d.ts`). It also exposes namespaces for managing organizations, applications, API keys, members, and per-app provider credentials on hosted multi-tenant instances.

## Authentication

- **API key** (server-side): pass `apiKey` (a `sk_live_…` secret) — sent as `Authorization: Bearer …`.
- **Session** (browser dashboards): omit `apiKey` and the client uses cookie-based sessions with CSRF handling.

## Links

- Website: https://litegen.ai
- Issues: https://github.com/visgotti/litegen-first/issues

## License

MIT
