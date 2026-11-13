// Ideogram — the OpenAPI specs behind the developer docs (developer.ideogram.ai,
// a Fern site since 2026-10). The docs are versioned: v2 (the default) is a new
// API, `POST /v2/{content}/{action}/{model}`; v1 is the API our adapter calls,
// and "keeps working" (ideogram-api/api-overview.md, "Moving from v1"). The old
// single spec at /openapi.json is now an HTML "Available APIs" picker.
//
// Where the specs live (moved 2026-10, re-read 2026-10-30): /.well-known/api-catalog
// maps each version's reference to its spec — v1 → /openapi/ideogram-api-v1.yaml,
// v2 → /openapi/ideogram-api-v2.yaml — each with a .json twin, which is what we
// read (/openapi.json, the "Available APIs" picker, links the same two .json
// files). The old Fern names (/openapi/api-reference-2.json for v1,
// /openapi/api-reference.json for v2) now 404. If the files move again, each
// source fails loudly (the v1 extract finds no /v1/ path, the v2 extract no
// /v2/ path), and the api-catalog says where they went.
// api.ideogram.ai/openapi.json now redirects to the v2 docs spec.
// Every v1 generate operation is now "(legacy)": "Legacy: use
// `POST /v2/image/generate/ideogram-4` instead" — no sunset date (2026-10-30).
//
// No model id is sent: the adapter POSTs the three 3.0 litegen ids to
// /v1/ideogram-v3/generate and the three 4.0 ids to /v1/ideogram-v4/generate,
// varying only `rendering_speed` (omitted / TURBO / QUALITY), so each id maps
// to the generation slug in its v1 path (what the adapter sends). That
// generation's v2 twin is acknowledged, not mapped: the adapter never sends it,
// so its going away is no desync (overview migration table:
// `/v1/ideogram-v4/generate` → `/v2/image/generate/ideogram-4`, `/v1/ideogram-v3/generate`
// → `/v2/image/generate/ideogram-3`).
//
// The ids a source lists are the model slugs of the text-to-image endpoints:
// - v1: /v1/{model}/generate (Ideogram 3.0, 4.0) and /v1/text-to-image/{model}
//   (P-Image), sync and async, transparent folded into its generation;
// - v2: /v2/image/generate/{model}, where every new model ships (Ideogram 4.5
//   is v2-only). An endpoint variant of a generation — `-transparent`,
//   `-custom-model`, `-character`, which v1 serves as a sibling endpoint or a
//   request field (custom_model_uri, character_reference_images) of that same
//   generation — folds into it, so a new generation such as ideogram-5 or a
//   resold model surfaces as new.
// The v1 spec's `V_n` enums are not used: ModelEnum only serves the legacy
// pre-3.0 /generate family, and its `V_4` is DescribeModelVersion (the
// /describe captioner), not a generation endpoint.
//
// Typings: the v1 request schemas of the two endpoints the adapter calls. A new
// rendering_speed, or FLASH becoming usable on 4.0 (the spec says it
// "currently return[s] a 400"), shows up in the snapshot diff. The v2 source
// snapshots its id list only — the adapter calls no v2 endpoint.
// Mapping mirrors litegen-core/src/providers/image/ideogram.rs `generation` and
// `rendering_speed`.
import { openapiExcerpt } from '../extract.mjs';

const ENDPOINTS = ['/v1/ideogram-v3/generate', '/v1/ideogram-v4/generate'];

const V1_GENERATE = [
  /^\/v1\/(?!async\/)([^/]+)\/(?:async\/)?generate(?:-transparent)?$/, // /v1/ideogram-v4/generate, …/async/generate-transparent
  /^\/v1\/(?:async\/)?text-to-image\/([^/]+)$/, // /v1/text-to-image/p-image-ideogram, /v1/async/text-to-image/…
];

// /v2/image/generate/ideogram-4-5, …/ideogram-3-character → ideogram-3
const V2_GENERATE = /^\/v2\/image\/generate\/([^/]+?)(?:-(?:transparent|custom-model|character))?$/;

const postPaths = (spec) =>
  Object.entries(spec.paths)
    .filter(([, ops]) => ops.post)
    .map(([path]) => path);

const v1GenerationModels = (spec) =>
  postPaths(spec).flatMap((path) => V1_GENERATE.map((re) => path.match(re)?.[1]).filter(Boolean));

// The v2 endpoints the adapter calls: Ideogram 4.5 Precise Edit
// (`/v2/image/precise-edit/{model}`, ids `precise-edit/{model}`) and, since
// 2026-11-03, Ideogram 4.5 generate (`/v2/image/generate/ideogram-4-5`).
const V2_PRECISE_EDIT = /^\/v2\/image\/precise-edit\/([^/]+)$/;
// 2026-11-04, the Nano Banana routes Ideogram resells (`/v2/image/generate/nano-banana-2`, `…/nano-banana-pro`).
const PRECISE_EDIT = [
  '/v2/image/precise-edit/ideogram-4-5',
  '/v2/image/generate/ideogram-4-5',
  '/v2/image/generate/nano-banana-2',
  '/v2/image/generate/nano-banana-pro',
  // 2026-11-04, P-Image (`/v2/image/generate/p-image-ideogram`, JSON).
  '/v2/image/generate/p-image-ideogram',
];

// 2026-11-10, the video models Ideogram resells: `/v2/video/generate/{model}-
// {text,image,reference}-to-video` (video/ideogram.rs), ids `video/{model}`.
const V2_VIDEO = /^\/v2\/video\/generate\/(.+?)-(?:text|image|reference)-to-video$/;
const VIDEO_MODELS = ['kling-3-standard', 'minimax-h3', 'seedance-2', 'seedance-2-5'];
const VIDEO_ENDPOINTS = VIDEO_MODELS.flatMap((m) =>
  ['text', 'image', ...(m.startsWith('kling') ? [] : ['reference'])].map((mode) => `/v2/video/generate/${m}-${mode}-to-video`),
);
const v2GenerationModels = (spec) => [
  ...postPaths(spec).map((path) => path.match(V2_GENERATE)?.[1]).filter(Boolean),
  ...postPaths(spec).map((path) => path.match(V2_PRECISE_EDIT)?.[1]).filter(Boolean).map((m) => `precise-edit/${m}`),
  ...postPaths(spec).map((path) => path.match(V2_VIDEO)?.[1]).filter(Boolean).map((m) => `video/${m}`),
];

// The price catalog's video section: each offering's per-second rates (US$
// micros), so a price change shows as a changed snapshot.
const videoPrices = (catalog) => {
  const video = catalog.public_pricing?.sections?.find((s) => s.id === 'video');
  if (!video) throw new Error('no "video" section in the usage-pricing catalog');
  const lines = [];
  for (const g of video.groups) {
    for (const o of g.offerings) {
      lines.push(`## ${o.id} (${o.endpoint?.path ?? 'no endpoint'})`);
      for (const r of o.usage_rates?.rows ?? []) {
        lines.push(`${r.label}: ${Object.entries(r.values).map(([k, v]) => `${k}=${v.amount}`).join(' ')}`);
      }
    }
  }
  return lines.join('\n') + '\n';
};

export default {
  provider: 'ideogram',
  models: {
    'ideogram/ideogram-v3': 'ideogram-v3',
    'ideogram/ideogram-v3-turbo': 'ideogram-v3',
    'ideogram/ideogram-v3-quality': 'ideogram-v3',
    'ideogram/ideogram-v4': 'ideogram-v4',
    'ideogram/ideogram-v4-turbo': 'ideogram-v4',
    'ideogram/ideogram-v4-quality': 'ideogram-v4',
    'ideogram/ideogram-4-5-precise-edit': 'precise-edit/ideogram-4-5',
    'ideogram/ideogram-4-5': 'ideogram-4-5',
    // Resold on v2, carried as Ideogram routes (2026-11-04).
    'ideogram/nano-banana-2': 'nano-banana-2',
    'ideogram/nano-banana-pro': 'nano-banana-pro',
    'ideogram/p-image-ideogram': 'p-image-ideogram',
    // Resold video on v2 (2026-11-10): its text, image and reference endpoints.
    'ideogram/kling-3-standard': 'video/kling-3-standard',
    'ideogram/minimax-h3': 'video/minimax-h3',
    'ideogram/seedance-2': 'video/seedance-2',
    'ideogram/seedance-2-5': 'video/seedance-2-5',
  },
  sources: [
    {
      // v1 API reference (the one the adapter calls).
      key: 'openapi',
      url: 'https://developer.ideogram.ai/openapi/ideogram-api-v1.json',
      expect: 'json',
      extract: v1GenerationModels,
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
    {
      // v2 API reference: where new generations ship.
      key: 'openapi-v2',
      url: 'https://developer.ideogram.ai/openapi/ideogram-api-v2.json',
      expect: 'json',
      extract: v2GenerationModels,
      snapshot: (spec, _body, { ours }) =>
        openapiExcerpt(spec, { paths: [...PRECISE_EDIT, ...VIDEO_ENDPOINTS], ours }) +
        [...PRECISE_EDIT, ...VIDEO_ENDPOINTS].join('\n') +
        '\n' +
        // Each carried video id beside the endpoints whose schemas type it.
        VIDEO_MODELS.map((m) => `video/${m}: ${VIDEO_ENDPOINTS.filter((p) => p.includes(`/${m}-`) && !p.slice(p.indexOf(m) + m.length).startsWith('-5')).join(' ')}`).join('\n') +
        '\n',
    },
    {
      // The price catalog behind ideogram.ai's pricing page (JSON).
      key: 'prices',
      index: true,
      url: 'https://ideogram.ai/api/usage-pricing/catalog',
      expect: 'json',
      extract: (catalog) => (catalog.public_pricing?.sections ?? []).flatMap((s) => s.groups.flatMap((g) => g.offerings.map((o) => o.id))),
      snapshot: videoPrices,
    },
  ],
  acknowledged: [
    ...['ideogram-3', 'ideogram-4'].map((id) => ({
      id,
      reason: 'v2 twin of a carried v1 generation (ideogram-v3 / ideogram-v4); the adapter calls v1, whose slug is the mapping',
    })),
    // Models Ideogram's v2 resells are carried as Ideogram routes (the user,
    // 2026-11-04: "like how litellm can still use openrouter"); these lack a
    // price litegen can bill.
    { id: 'auto', reason: 'skipped 2026-11-03: v2 "automatic model selection" runs a model Ideogram picks, priced by that model; litegen carries each model by id' },
    ...['gpt-image-2', 'gpt-image-2-5-flare', 'gpt-image-2-5-sunburst'].map((id) => ({
      id,
      reason: "no price litegen can quote (read 2026-11-04 from https://ideogram.ai/api/usage-pricing/catalog): Ideogram bills OpenAI's tokens as used, the request sets no quality, and its own estimate at 1024² runs from US $0.0069 (low) to US $0.2118 (high)",
    })),
    { id: 'nano-banana-2-1', reason: 'skipped 2026-11-04: Ideogram\'s price page lists Nano Banana 2 and Pro, not 2.1' },
    ...['ideogram-2', 'ideogram-2a'].map((id) => ({ id, reason: 'skipped 2026-11-03: legacy Ideogram 2.x on v2; 3.0, 4.0 and 4.5 are carried' })),
    { id: 'z-image', reason: 'skipped 2026-11-03: a third-party model resold through Ideogram v2, with no Ideogram price in its served pages' },
  ],
};
