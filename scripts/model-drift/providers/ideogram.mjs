// Ideogram — the OpenAPI specs behind the developer docs (developer.ideogram.ai,
// a Fern site since 2026-10). The docs are versioned: v2 (the default) is a new
// API, `POST /v2/{content}/{action}/{model}`; v1 is the API our adapter calls,
// and "keeps working" (ideogram-api/api-overview.md, "Moving from v1"). The old
// single spec at /openapi.json is now an HTML "Available APIs" picker.
//
// Where the specs live: /.well-known/api-catalog maps each version's reference
// to its spec — v1 → /openapi/api-reference-2.yaml, v2 → /openapi/api-reference.yaml,
// each with a .json twin, which is what we read. The `-2` is Fern's numbering;
// if the two ever swap, each source fails loudly (the v1 extract finds no
// /v1/ path, the v2 extract no /v2/ path), and the catalog says where they went.
// api.ideogram.ai/openapi.json is the server's own spec with undocumented and
// partner endpoints (ernie, z-image, fp8/distilled builds); we track the
// documented API only.
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

const v2GenerationModels = (spec) => postPaths(spec).map((path) => path.match(V2_GENERATE)?.[1]).filter(Boolean);

export default {
  provider: 'ideogram',
  models: {
    'ideogram/ideogram-v3': 'ideogram-v3',
    'ideogram/ideogram-v3-turbo': 'ideogram-v3',
    'ideogram/ideogram-v3-quality': 'ideogram-v3',
    'ideogram/ideogram-v4': 'ideogram-v4',
    'ideogram/ideogram-v4-turbo': 'ideogram-v4',
    'ideogram/ideogram-v4-quality': 'ideogram-v4',
  },
  sources: [
    {
      // v1 API reference (the one the adapter calls).
      key: 'openapi',
      url: 'https://developer.ideogram.ai/openapi/api-reference-2.json',
      expect: 'json',
      extract: v1GenerationModels,
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
    {
      // v2 API reference: where new generations ship.
      key: 'openapi-v2',
      url: 'https://developer.ideogram.ai/openapi/api-reference.json',
      expect: 'json',
      extract: v2GenerationModels,
    },
  ],
  acknowledged: [
    ...['ideogram-3', 'ideogram-4'].map((id) => ({
      id,
      reason: 'v2 twin of a carried v1 generation (ideogram-v3 / ideogram-v4); the adapter calls v1, whose slug is the mapping',
    })),
    { id: 'p-image-ideogram', reason: 'skipped 2026-09-11: P-Image is a budget tier on /v1/text-to-image/{model}; it needs its own request branch' },
  ],
};
