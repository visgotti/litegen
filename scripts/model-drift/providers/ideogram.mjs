// Ideogram — the OpenAPI spec behind the developer docs (developer.ideogram.ai;
// its llms.txt names https://developer.ideogram.ai/openapi.json as the raw spec).
// api.ideogram.ai/openapi.json is the server's own 880KB spec with undocumented
// and partner endpoints (ernie, z-image, fp8/distilled builds); we track the
// documented API only.
//
// No model id is sent: the adapter POSTs all three litegen ids to
// /v1/ideogram-v3/generate and varies only `rendering_speed` (omitted / TURBO /
// QUALITY), so all three map to the generation slug in that path. The ids a
// source lists are the model slugs of the text-to-image endpoints the docs'
// "Generate Images" page offers — /v1/{model}/generate (Ideogram 3.0, 4.0) and
// /v1/text-to-image/{model} (P-Image) — so a new generation such as
// /v1/ideogram-v5/generate surfaces as new. The spec's `V_n` enums are not used:
// ModelEnum only serves the legacy pre-3.0 /generate family, and its `V_4` is
// DescribeModelVersion (the /describe captioner), not a generation endpoint.
// A new rendering_speed (e.g. FLASH) shows up in the snapshot diff.
// Mapping mirrors litegen-core/src/providers/image/ideogram.rs `rendering_speed`.
import { openapiExcerpt } from '../extract.mjs';

const ENDPOINT = '/v1/ideogram-v3/generate';

const GENERATE = [
  /^\/v1\/(?!async\/)([^/]+)\/(?:async\/)?generate(?:-transparent)?$/, // /v1/ideogram-v4/generate, …/async/generate-transparent
  /^\/v1\/(?:async\/)?text-to-image\/([^/]+)$/, // /v1/text-to-image/p-image-ideogram, /v1/async/text-to-image/…
];

const generationModels = (spec) =>
  Object.entries(spec.paths)
    .filter(([, ops]) => ops.post)
    .flatMap(([path]) => GENERATE.map((re) => path.match(re)?.[1]).filter(Boolean));

export default {
  provider: 'ideogram',
  models: {
    'ideogram/ideogram-v3': 'ideogram-v3',
    'ideogram/ideogram-v3-turbo': 'ideogram-v3',
    'ideogram/ideogram-v3-quality': 'ideogram-v3',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://developer.ideogram.ai/openapi.json',
      expect: 'json',
      extract: generationModels,
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: [ENDPOINT], ours }),
    },
  ],
  acknowledged: [],
};
