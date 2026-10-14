// Recraft — three views of the one endpoint our adapter calls (POST
// /v1/images/generations, body `model`; GET /v1/users/me is only a health
// check), plus the docs index:
//  • openapi: the OpenAPI spec the API host serves behind its Swagger UI
//    (docs/api-reference/swagger → external.api.recraft.ai/doc →
//    doc/ui/?spec=external-api.yaml → doc/spec/external-api.yaml). It is YAML,
//    read by extract.mjs's strict block-YAML reader (the drift core has no
//    YAML dependency), then excerpted like any other OpenAPI spec. Its `model`
//    enum lags the docs (it had no recraftv4_1_flash on 2026-10-12).
//  • models: the docs' one-page API summary for agents (agents.md). Its
//    "Models" table is the docs' list of every `model` value, with output,
//    price and prompt limit; the snapshot keeps that table, the per-family
//    parameter-support table, and the Generate image field and exact-size
//    tables.
//  • endpoints: the docs' markdown twin of the Endpoints page — the "Generate
//    image" parameter table (names, types/defaults, per-model compatibility).
//    Its `model` row stopped listing values in 2026-10 (it links the All
//    models page instead), so its only id is the documented default.
//  • pages: llms.txt's per-model API pages, so a new model family shows up.
// Mapping mirrors litegen-core/src/providers/image/recraft.rs (strip `recraft/`).
import { openapiModelIds, openapiExcerpt, parseYaml } from '../extract.mjs';

const ENDPOINTS = ['/v1/images/generations'];
const DOCS = 'https://www.recraft.ai/docs/api-reference';

// Both callbacks of the openapi source parse the same body; parse it once.
let cached = { body: null, doc: null };
const spec = (body) => {
  if (cached.body !== body) cached = { body, doc: parseYaml(body) };
  return cached.doc;
};

/** The text under the first heading matching `start`, up to the next heading matching `end`. */
function slice(text, start, end, page) {
  const from = text.search(start);
  if (from < 0) throw new Error(`no ${start} heading — the ${page} page was restructured`);
  const rest = text.slice(from).replace(start, '');
  const to = rest.search(end);
  return to < 0 ? rest : rest.slice(0, to);
}

const cells = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());

/** Every pipe table in `text`, each as { header, rows } of trimmed cells (the |-|-| rule dropped). */
function tables(text) {
  const out = [];
  let lines = [];
  for (const line of [...text.split('\n'), '']) {
    if (line.startsWith('|')) {
      lines.push(cells(line));
      continue;
    }
    if (lines.length) {
      const [header, , ...rows] = lines;
      out.push({ header, rows });
      lines = [];
    }
  }
  return out;
}

const asObjects = ({ header, rows }) => rows.map((r) => Object.fromEntries(header.map((h, k) => [h, r[k] ?? ''])));

/** The rows of the Endpoints page's "Generate image" → "Parameters" table, as { column header: cell } objects. */
function generateImageParams(md) {
  const params = slice(slice(md, /^## Generate image[ \t]*$/m, /^## /m, 'Endpoints'), /^### Parameters[ \t]*$/m, /^#{2,3} /m, 'Endpoints');
  const [table] = tables(params);
  if (!table?.header.includes('Parameter')) throw new Error('Generate image parameters table has no Parameter column');
  return asObjects(table);
}

// agents.md: the "## Models" section opens with the model table, whose first
// column is headed `model`; the "## Generate image" section holds the field
// and exact-size tables.
const agentsSection = (md, heading) => slice(md, new RegExp(`^## ${heading}[ \\t]*$`, 'm'), /^## /m, 'agents');

/**
 * The `model` values of agents.md's Models table: every backticked token of
 * the first column (the id, then any alias it names, e.g. "`recraftv4_1_flash`
 * (alias `recraftv4_1_flash_raster`)"). A cell that does not open with a
 * backticked id means the table was rewritten into prose, so it throws rather
 * than reading words out of a sentence.
 */
function modelTableIds(md) {
  const table = tables(agentsSection(md, 'Models'))[0];
  if (table?.header[0] !== '`model`') throw new Error('agents.md "Models" section no longer opens with a `model` table');
  return table.rows.flatMap((r) => {
    if (!/^`[^`]+`/.test(r[0])) throw new Error(`agents.md Models table row is not a model id: ${r[0].slice(0, 60)}`);
    const ids = [...r[0].matchAll(/`([^`]+)`/g)].map((m) => m[1]);
    const bad = ids.find((id) => !/^[a-z][a-z0-9_.-]*$/.test(id));
    if (bad) throw new Error(`agents.md Models table has a non-id value \`${bad}\``);
    return ids;
  });
}

/** agents.md's typings tables, cell for cell, minus the prose "Notes" column. */
function agentsTables(md) {
  const out = [];
  for (const heading of ['Models', 'Generate image']) {
    for (const { header, rows } of tables(agentsSection(md, heading))) {
      const keep = header.map((h, k) => (h === 'Notes' ? -1 : k)).filter((k) => k >= 0);
      out.push(`# ${heading}: ${keep.map((k) => header[k]).join(' | ')}`);
      for (const r of rows) out.push(keep.map((k) => r[k] ?? '').join(' | '));
      out.push('');
    }
  }
  return out.join('\n');
}

export default {
  provider: 'recraft',
  models: {
    'recraft/recraftv3': 'recraftv3',
    'recraft/recraftv3_vector': 'recraftv3_vector',
    'recraft/recraftv2': 'recraftv2',
    'recraft/recraftv4_1': 'recraftv4_1',
    'recraft/recraftv4_1_pro': 'recraftv4_1_pro',
    'recraft/recraftv4_1_vector': 'recraftv4_1_vector',
    'recraft/recraftv4_1_pro_vector': 'recraftv4_1_pro_vector',
    'recraft/recraftv2_vector': 'recraftv2_vector',
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
      key: 'models',
      url: `${DOCS}/agents.md`,
      expect: 'text',
      extract: (_md, body) => modelTableIds(body),
      snapshot: (_md, body) => agentsTables(body),
    },
    {
      key: 'endpoints',
      url: `${DOCS}/endpoints.md`,
      expect: 'text',
      // The `model` row's documented default ("string or null, default is `recraftv4_1`").
      extract: (_md, body) => {
        const row = generateImageParams(body).find((r) => r.Parameter.replace(/\\/g, '') === 'model');
        if (!row) throw new Error('Generate image parameters table has no `model` row');
        const fallback = (row.Type ?? '').match(/\bdefault is `([^`]+)`/);
        if (!fallback) throw new Error('Generate image `model` row no longer names a default');
        return [fallback[1]];
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
        'explicit-raster spelling of the undecorated id (docs: "every raster model also accepts a `_raster` alias of the same model"); the OpenAPI enum and the agents.md Models table list these',
    },
    // recraftv4, recraftv4_vector, recraftv4_pro, recraftv4_pro_vector — not recraftv4_1*.
    {
      pattern: /^recraftv4(?:_pro)?(?:_vector)?$/,
      reason: 'skipped 2026-09-11: the V4 (non-.1) generation; V4.1 is carried',
    },
    // recraftv4_1_utility, _utility_vector, _utility_pro, _utility_pro_vector.
    {
      pattern: /^recraftv4_1_utility(?:_pro)?(?:_vector)?$/,
      reason: 'skipped 2026-09-11: the V4.1 utility variants (the predictability-tuned twins of V4.1, same prices); not carried',
    },
    // recraftv4_styles, _styles_vector, _styles_pro, _styles_pro_vector.
    {
      pattern: /^recraftv4_styles(?:_pro)?(?:_vector)?$/,
      reason: 'skipped 2026-09-11: the V4 Styles family always requires a style (style_id or style references), a workflow we do not expose',
    },
  ],
};
