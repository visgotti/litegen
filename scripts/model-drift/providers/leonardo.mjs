// Leonardo — our adapters call the v1 REST API: POST v1/generations with a
// platform-model UUID in `modelId`, and POST v1/generations-image-to-video
// with a `model` enum. Leonardo's current docs describe only its v2 API, so
// the v1 pages come from the versioned docs (docs.leonardo.ai/v1.0/…): `.md`
// twins of ReadMe pages, each embedding its endpoint's OpenAPI definition as
// a ```json block.
//  • image-to-video: the v1 reference's `model` enum.
//  • model-ids: "Refer to Commonly Used API Values" — image-model UUIDs (the
//    snapshot keeps the names beside them, so a new UUID in the report can be
//    looked up) and video `model` values, including ones the reference enum
//    lags on (VEO3_1, which the v1 Veo 3.1 guide sends to image-to-video).
//  • generations: the v1 image reference; its only model id is the `modelId`
//    default, the snapshot is the request schema.
//  • deprecations (index): retirements are announced on this page while the
//    v1 pages keep listing the retired ids, so a new notice shows up here.
//  • v2-models (index): the v2 API's model lineup — a different request shape
//    our adapters do not speak — so its changes show up as a snapshot diff.
// Mapping mirrors litegen-core/src/providers/image/leonardo.rs
// (`resolve_model_id`) and video/leonardo.rs (`resolve_model`).
import { markdownExcerpt, openapiModelIds, openapiExcerpt } from '../extract.mjs';

const V1 = 'https://docs.leonardo.ai/v1.0';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const embedded = (_md, body) => JSON.parse(markdownExcerpt(body, { lang: 'json' }));
const cells = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim().replace(/\\(.)/g, '$1'));

/** The table under a `### <heading>`, as { column header: cell } rows. */
function table(md, heading) {
  const at = md.search(new RegExp(`^#{2,4} ${heading}[ \\t]*$`, 'm'));
  if (at < 0) throw new Error(`no "${heading}" table`);
  const lines = [];
  for (const line of md.slice(at).split('\n').slice(1)) {
    if (line.startsWith('|')) lines.push(line);
    else if (lines.length) break;
  }
  const [header, , ...rows] = lines.map(cells);
  if (!header) throw new Error(`"${heading}" has no table`);
  return rows.map((r) => Object.fromEntries(header.map((h, k) => [h, r[k] ?? ''])));
}
const column = (rows, name, heading) => {
  if (rows.length && !(name in rows[0])) throw new Error(`"${heading}" table has no "${name}" column`);
  return rows.map((r) => r[name]);
};
const imageModels = (md) => table(md, 'Image Model IDs');
const videoModels = (md) => table(md, 'Video Model IDs');

const v1Endpoint = (key, slug, path, extract) => ({
  key,
  url: `${V1}/reference/${slug}.md`,
  expect: 'text',
  extract: (md, body) => extract(embedded(md, body)),
  snapshot: (md, body, { ours }) => openapiExcerpt(embedded(md, body), { paths: [path], ours }),
});

export default {
  provider: 'leonardo',
  models: {
    // Leonardo Diffusion XL (b24e16ff-… is Lightning XL, the API's modelId default).
    'leonardo/diffusion-xl': '1e60896f-3c26-4296-8ecc-53e2afecc132',
    'leonardo/motion2': 'MOTION2',
    'leonardo/veo3': 'VEO3',
    'leonardo/kling2.1': 'KLING2_1',
  },
  sources: [
    v1Endpoint('image-to-video', 'createimagetovideogeneration', '/generations-image-to-video', (spec) =>
      openapiModelIds(spec, { paths: ['/generations-image-to-video'] }),
    ),
    {
      key: 'model-ids',
      url: `${V1}/docs/commonly-used-api-values.md`,
      expect: 'text',
      // Only well-formed UUIDs: the table has a truncated one (DreamShaper v7) the API cannot accept.
      extract: (_md, body) => [
        ...column(imageModels(body), 'Model ID', 'Image Model IDs').filter((id) => UUID.test(id)),
        ...column(videoModels(body), 'Model', 'Video Model IDs'),
      ],
      snapshot: (_md, body) =>
        [
          '# image models',
          ...imageModels(body).map((r) => `${r['Model ID']}  ${r.Model}`),
          '',
          '# video models',
          ...videoModels(body).map((r) => `${r.Model}  ${r['Video Model']}`),
        ].join('\n'),
    },
    v1Endpoint('generations', 'creategeneration', '/generations', (spec) => {
      const modelId = spec.paths?.['/generations']?.post?.requestBody?.content?.['application/json']?.schema?.properties?.modelId;
      if (!modelId) throw new Error('POST /generations has no `modelId` request property');
      return [modelId.default];
    }),
    {
      key: 'deprecations',
      index: true,
      url: 'https://docs.leonardo.ai/docs/deprecations-changes.md',
      expect: 'text',
      extract: /^## (.+?)\s*$/gm,
    },
    {
      key: 'v2-models',
      index: true,
      url: 'https://docs.leonardo.ai/reference/creategeneration.md',
      expect: 'text',
      extract: (md, body) => openapiModelIds(embedded(md, body), { paths: ['/generations'] }),
    },
  ],
  acknowledged: [
    {
      id: 'VEO3FAST',
      reason: 'Veo 3 Fast retired June 29, 2026 (Deprecations & Changes page); the v1 pages still list it',
    },
  ],
};
