# @litegen/sdk

First-party TypeScript/JavaScript SDK for **[LiteGen](https://litegen.ai)** — one API for every AI image, video & 3D generation model. Bring your own provider keys, or use a hosted LiteGen instance, and call dozens of image, video and 3D models through a single typed client.

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

## 3D models

3D generation is always async: `models3d.generate` returns a job that is awaitable (the final state) and async-iterable (progress updates), like `videos.generate`.

```ts
// Text-to-3D. Every format in `output_formats` is guaranteed on a completed job.
const mesh = await litegen.models3d.generate({
  model: 'fal/hyper3d-rodin',
  prompt: 'a weathered bronze statue of a fox',
  output_formats: ['glb', 'obj'],
});
if (mesh.status === 'completed') {
  const glb = mesh.assets?.find((a) => a.kind === 'mesh' && a.format === 'glb');
}

// Image-to-3D: the image goes in `reference_images` under the `init` role.
// Image-only models such as fal/trellis take no prompt.
for await (const update of litegen.models3d.generate({
  model: 'fal/trellis',
  reference_images: [{ type: 'url', value: 'https://example.com/chair.png', role: 'init' }],
})) {
  console.log(`${update.status} — ${update.progress}%`);
}
```

- **What a model accepts:** `litegen.models.get(id)` returns its params with their allowed values, its reference-image roles (`init`, or `view-front` / `view-back` / … for multiview) and its prompt rule (`prompt.max_length === 0` means it takes none). `litegen.models.list({ media_type: 'model3d' })` lists the 3D catalog.
- **Formats:** `output_formats` is typed with `Model3dFormat`, the full container vocabulary. What one model can deliver is its `params.output_formats.enum_values`; the containers litegen converts locally are its `derivable_values`, and any subset of those costs one provider job — so `max_items` counts provider jobs, not files. Read both from `models.get(id)` rather than hard-coding them: they grow as litegen learns new containers.
- **Lax mode:** with `strict: false` the server drops params the model does not support instead of rejecting the request; `await job.droppedParams` lists them.
- **Converting a mesh you already have:** `litegen.models3d.convert(bytes, { from: 'glb', to: 'stl' })` returns `{ blob, format, filename, losses }` — a standalone utility, nothing generated or billed. `losses` names what the target container could not keep (e.g. `uvs`). `litegen.models3d.convertFormats()` lists the containers it handles and its size cap.
- **Polling and cancelling:** 3D polls wait up to 30 minutes by default (`MODEL3D_DEFAULT_TIMEOUT_MS`; pass `{ timeoutMs }` to change it). Cancel a pending job with `litegen.generations.cancel(id)`.

See [`examples/generate-3d.ts`](./examples/generate-3d.ts).

## Authentication

- **API key** (server-side): pass `apiKey` (a `sk_live_…` secret) — sent as `Authorization: Bearer …`.
- **Session** (browser dashboards): omit `apiKey` and the client uses cookie-based sessions with CSRF handling.

## Links

- Website: https://litegen.ai
- Issues: https://github.com/visgotti/litegen-first/issues

## License

MIT
