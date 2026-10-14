// Leonardo — our adapters call the v1 REST API: POST v1/generations with a
// platform-model UUID in `modelId`, POST v1/generations-image-to-video with a
// `model` enum, and POST v1/init-image (presigned upload of a reference or
// start-frame image) for both. They never call v2 (POST v2/generations with a
// `model` + `parameters` body). Leonardo's current API reference describes
// only v2, so the v1 references come from the versioned docs
// (docs.leonardo.ai/v1.0/…): `.md` twins of ReadMe pages, each embedding its
// endpoint's OpenAPI definition as a ```json block. The versioned docs are
// frozen (no retirement notices), so a guide the current docs still keep in
// v1 shape is read from the current docs.
//  • image-to-video: the v1 reference's `model` enum.
//  • model-ids: "Refer to Commonly Used API Values" — image-model UUIDs (the
//    snapshot keeps the names beside them, so a new UUID in the report can be
//    looked up) and video `model` values, including ones the reference enum
//    lags on (VEO3_1 and VEO3_1FAST, which the v1 Veo 3.1 guide sends to
//    image-to-video).
//  • veo31-guide, kling21-guide, kling25-guide: the per-model guides for the
//    image-to-video models we carry — the only place each model's v1 `model`
//    value, duration set and resolution presets are documented ("Set to `4`,
//    `6`, or `8`"). Ids from the Body Parameters table's `model` row; the
//    snapshot is the documented endpoint, any warning callout (Leonardo
//    announced the Kling 2.1 Pro retirement only there), and that table's
//    names, types and backticked values. Motion 2.0 has no v1 guide (only
//    recipes), so the reference schema covers it.
//  • generations: the v1 image reference; its only model id is the `modelId`
//    default, the snapshot is the request schema.
//  • init-image (index): the v1 upload reference — no model, but the adapters
//    call it before every reference-image request.
//  • deprecations (index): retirements are announced on this page while the
//    v1 pages keep listing the retired ids, so a new notice shows up here.
//  • v2-models (index): the v2 API's model lineup — a different request shape
//    our adapters do not speak — so its changes show up as a snapshot diff.
//    The snapshot also keeps the v2 parameters of our video models' twins:
//    models/leonardo.yaml takes their prompt limits from those schemas.
// Mapping mirrors litegen-core/src/providers/image/leonardo.rs
// (`resolve_model_id`) and video/leonardo.rs (`resolve_model`).
import { markdownExcerpt, openapiModelIds, openapiExcerpt } from '../extract.mjs';

const CURRENT = 'https://docs.leonardo.ai';
const V1 = `${CURRENT}/v1.0`;
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
const ticks = (cell) => [...cell.matchAll(/`([^`]+)`/g)].map((m) => m[1]);

/** A v1 guide's Body Parameters table, checked for the columns it is read by. */
function guideParams(md, slug) {
  const rows = table(md, 'Body Parameters');
  for (const name of ['Parameter', 'Type', 'Definition']) column(rows, name, `${slug} Body Parameters`);
  return rows;
}

/** Warning callouts, whitespace-collapsed: some retirements are announced only here (Kling 2.1 Pro, 2026-09-15). */
const notices = (md) => [...md.matchAll(/<Callout\b[^>]*theme="warn"[^>]*>([\s\S]*?)<\/Callout>/g)].map((m) => m[1].replace(/\s+/g, ' ').trim());

/** The URL in the guide's "## Endpoint" code block: which API (v1 image-to-video or v2) it documents. */
function guideEndpoint(md, slug) {
  const m = md.match(/^## Endpoint[ \t]*\n+```[^\n]*\n\s*(\S+)/m);
  if (!m) throw new Error(`${slug}: no "## Endpoint" code block`);
  return m[1];
}

const guide = (key, base, slug) => ({
  key,
  url: `${base}/docs/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => {
    const row = guideParams(body, slug).find((r) => r.Parameter === 'model');
    if (!row) throw new Error(`${slug}: Body Parameters table has no \`model\` row`);
    return ticks(row.Definition);
  },
  // The Definition column is prose; its backticked values are the accepted ones.
  snapshot: (_md, body) =>
    [
      `endpoint: ${guideEndpoint(body, slug)}`,
      ...notices(body).map((n) => `notice: ${n}`),
      '',
      ...guideParams(body, slug).map((r) => [r.Parameter, r.Type, ticks(r.Definition).join(' ')].join(' | ')),
    ].join('\n'),
});

// The v2 twins of the video models we carry (MOTION2, VEO3_1, VEO3_1FAST,
// KLING2_5): models/leonardo.yaml takes prompt limits from their schemas.
// KLING2_1's twin, kling-2.1, left the v2 lineup in 2026-10.
const V2_TWINS = ['motion_2.0', 'veo-3.1-generate-001', 'veo-3.1-fast-generate-001', 'kling-2.5'];
const CONSTRAINTS = ['type', 'enum', 'const', 'default', 'minimum', 'maximum', 'minLength', 'maxLength', 'minItems', 'maxItems'];
const constraints = (p) => JSON.stringify(Object.fromEntries(CONSTRAINTS.filter((k) => k in p).map((k) => [k, p[k]])));

/** One line per `parameters` property (and its sub-properties) of each twin's v2 request branch. */
function twinParameters(spec) {
  const [schema] = Object.values(JSON.parse(openapiExcerpt(spec, { paths: ['/generations'], ours: V2_TWINS })));
  const lines = [];
  for (const branch of schema?.oneOf ?? []) {
    const model = branch.properties?.model?.enum?.[0] ?? branch.properties?.model?.const;
    for (const [name, p] of Object.entries(branch.properties?.parameters?.properties ?? {})) {
      lines.push(`${model} ${name} ${constraints(p)}`);
      for (const [sub, q] of Object.entries(p.properties ?? {})) lines.push(`${model} ${name}.${sub} ${constraints(q)}`);
    }
  }
  if (!lines.length) throw new Error("v2 POST /generations: no twin of our models has a `parameters` schema — the spec was restructured");
  return lines.join('\n');
}

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
    // The current docs' Kling guides still document the v1 request (and carry
    // retirement callouts); the current Veo 3.1 guide documents v2, so its
    // v1 copy is read.
    guide('veo31-guide', V1, 'veo-31'),
    guide('kling21-guide', CURRENT, 'kling-2-1-pro'),
    guide('kling25-guide', CURRENT, 'kling-2-5-turbo'),
    v1Endpoint('generations', 'creategeneration', '/generations', (spec) => {
      const modelId = spec.paths?.['/generations']?.post?.requestBody?.content?.['application/json']?.schema?.properties?.modelId;
      if (!modelId) throw new Error('POST /generations has no `modelId` request property');
      return [modelId.default];
    }),
    { ...v1Endpoint('init-image', 'uploadinitimage', '/init-image', (spec) => Object.keys(spec.paths)), index: true },
    {
      key: 'deprecations',
      index: true,
      url: `${CURRENT}/docs/deprecations-changes.md`,
      expect: 'text',
      extract: /^## (.+?)\s*$/gm,
    },
    {
      key: 'v2-models',
      index: true,
      url: `${CURRENT}/reference/creategeneration.md`,
      expect: 'text',
      extract: (md, body) => openapiModelIds(embedded(md, body), { paths: ['/generations'] }),
      snapshot: (md, body) => {
        const spec = embedded(md, body);
        const ids = [...new Set(openapiModelIds(spec, { paths: ['/generations'] }).map((id) => id.toLowerCase()))].sort();
        return `# v2 models\n${ids.join('\n')}\n\n# v2 parameters of our models' twins\n${twinParameters(spec)}`;
      },
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
