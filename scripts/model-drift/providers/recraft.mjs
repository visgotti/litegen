// Recraft — two views of the one endpoint our adapter calls (POST
// /v1/images/generations, body `model`), plus the docs index:
//  • openapi: the OpenAPI spec the API host serves behind its Swagger UI
//    (docs/api-reference/swagger → external.api.recraft.ai/doc →
//    doc/ui/?spec=external-api.yaml → doc/spec/external-api.yaml). It is YAML,
//    read by extract.mjs's strict block-YAML reader (the drift core has no
//    YAML dependency), then excerpted like any other OpenAPI spec.
//  • endpoints: the docs' markdown twin of the Endpoints page — the "Generate
//    image" parameter table, whose `model` row lists every accepted value.
//  • pages: llms.txt's per-model API pages, so a new model family shows up.
// Mapping mirrors litegen-core/src/providers/image/recraft.rs (strip `recraft/`).
import { openapiModelIds, openapiExcerpt, parseYaml } from '../extract.mjs';

const ENDPOINTS = ['/v1/images/generations'];

// Both callbacks of the openapi source parse the same body; parse it once.
let cached = { body: null, doc: null };
const spec = (body) => {
  if (cached.body !== body) cached = { body, doc: parseYaml(body) };
  return cached.doc;
};

/** The rows of the "Generate image" → "Parameters" table, as { column header: cell } objects. */
function generateImageParams(md) {
  const slice = (text, start, end) => {
    const from = text.search(start);
    if (from < 0) throw new Error(`no ${start} heading — the Endpoints page was restructured`);
    const rest = text.slice(from).replace(start, '');
    const to = rest.search(end);
    return to < 0 ? rest : rest.slice(0, to);
  };
  const params = slice(slice(md, /^## Generate image[ \t]*$/m, /^## /m), /^### Parameters[ \t]*$/m, /^#{2,3} /m);
  const cells = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());
  const [header, , ...rows] = params.split('\n').filter((l) => l.startsWith('|')).map(cells);
  if (!header?.includes('Parameter')) throw new Error('Generate image parameters table has no Parameter column');
  return rows.map((r) => Object.fromEntries(header.map((h, k) => [h, r[k] ?? ''])));
}

export default {
  provider: 'recraft',
  models: {
    'recraft/recraftv3': 'recraftv3',
    'recraft/recraftv3_vector': 'recraftv3_vector',
    'recraft/recraftv2': 'recraftv2',
    'recraft/recraftv4_1': 'recraftv4_1',
    'recraft/recraftv4_1_pro': 'recraftv4_1_pro',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://external.api.recraft.ai/doc/spec/external-api.yaml',
      expect: 'text',
      extract: (_text, body) => openapiModelIds(spec(body), { paths: ENDPOINTS }),
      snapshot: (_text, body, { ours }) =>
        `# endpoints\n${Object.keys(spec(body).paths).sort().join('\n')}\n\n` + openapiExcerpt(spec(body), { paths: ENDPOINTS, ours }),
    },
    {
      key: 'endpoints',
      url: 'https://www.recraft.ai/docs/api-reference/endpoints.md',
      expect: 'text',
      extract: (_md, body) => {
        const row = generateImageParams(body).find((r) => r.Parameter.replace(/\\/g, '') === 'model');
        if (!row) throw new Error('Generate image parameters table has no `model` row');
        return [...(row.Compatibility ?? '').matchAll(/`([^`]+)`/g)].map((m) => m[1]);
      },
      // Parameter names, types/defaults and allowed values; the Description column is prose.
      snapshot: (_md, body) =>
        generateImageParams(body)
          .map((r) => [r.Parameter, r.Type, r.Compatibility].join(' | '))
          .join('\n'),
    },
    {
      key: 'pages',
      index: true,
      url: 'https://www.recraft.ai/docs/llms.txt',
      expect: 'text',
      extract: /\/docs\/api-reference\/models\/[\w.-]+\.md/g,
    },
  ],
  acknowledged: [
    {
      pattern: /_raster$/,
      reason:
        'explicit-raster spelling of the undecorated id (docs: every model not ending in _vector is raster); only the OpenAPI enum lists these, the docs model tables do not',
    },
  ],
};
