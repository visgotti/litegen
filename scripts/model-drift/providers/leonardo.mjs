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
//    lags on (VEO3_1 and VEO3_1FAST, which the v1 Veo 3.1 guide sends to
//    image-to-video).
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
    'leonardo/veo3.1': 'VEO3_1',
    'leonardo/veo3.1-fast': 'VEO3_1FAST',
    'leonardo/kling2.1': 'KLING2_1',
    'leonardo/kling2.5': 'KLING2_5',
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
      id: 'VEO3',
      reason: 'Veo 3 retired June 29, 2026 (Deprecations & Changes page); leonardo/veo3 removed 2026-09-11, replaced by leonardo/veo3.1; the v1 pages still list it',
    },
    {
      id: 'VEO3FAST',
      reason: 'Veo 3 Fast retired June 29, 2026 (Deprecations & Changes page); the v1 pages still list it',
    },
    { id: 'MOTION2FAST', reason: 'skipped 2026-09-11: each needs a per-model mapping; unmapped names become Lightning XL' },
    // Platform image models (names from the model-ids snapshot). The image
    // adapter passes only leonardo/diffusion-xl through a mapping; any other
    // catalog name would silently generate with Lightning XL.
    ...[
      '7b592283-e8a7-4c5a-9ba6-d18c31f258b9', // Lucid Origin
      '05ce0082-2d80-4a2d-8653-4d1c85e2418e', // Lucid Realism
      '28aeddf8-bd19-4803-80fc-79602d1a9989', // FLUX.1 Kontext
      'de7d3faf-762f-48e0-b3b7-9d0ac3a3fcf3', // Leonardo Phoenix 1.0
      'b2614463-296c-462a-9586-aafdb8f00e36', // Flux Dev
      '1dd50843-d653-4516-a8e3-f0238ee453ff', // Flux Schnell
      '6b645e3a-d64f-4341-a6d8-7a3690fbf042', // Leonardo Phoenix 0.9
      'e71a1c2f-4f80-4800-934f-2c68979d8cc8', // Leonardo Anime XL
      'b24e16ff-06e3-43eb-8d33-4416c2d75876', // Leonardo Lightning XL
      '16e7060a-803e-4df3-97ee-edcfa5dc9cc8', // SDXL 1.0
      'aa77f04e-3eec-4034-9c07-d0f619684628', // Leonardo Kino XL
      '5c232a9e-9061-4777-980a-ddc8e65647c6', // Leonardo Vision XL
      '2067ae52-33fd-4a82-bb92-c2c55e7d2786', // AlbedoBase XL
      'f1929ea3-b169-4c18-a16c-5d58b4292c69', // RPG v5
      'b63f7119-31dc-4540-969b-2a9df997e173', // SDXL 0.9
      'd69c8273-6b17-4a30-a13e-d6637ae1c644', // 3D Animation Style
    ].map((id) => ({ id, reason: 'skipped 2026-09-11: each needs a per-model mapping; unmapped names become Lightning XL' })),
  ],
};
