// Vidu — the docs site serves a markdown twin of every page (`<page>.md`,
// indexed by llms.txt). Sources: the Model Map's video tables (the lineup, with
// resolution/duration per model), and the four endpoint pages our adapter
// routes between, whose Request Body `model` row lists the accepted values.
// Ids come from table structure and those value lists only, so marketing names
// in prose and headings ("ViduQ3 Series") are not counted.
// Mapping mirrors litegen-core/src/providers/video/vidu.rs (strip `vidu/`).

const DOCS = 'https://platform.vidu.com/docs';
const API_ID = /\bvidu[a-z0-9][\w.-]*/gi;

const cells = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());
const tables = (md) =>
  md
    .split(/\n(?!\|)/)
    .map((chunk) => chunk.split('\n').filter((l) => l.startsWith('|')).map(cells))
    .filter((t) => t.length >= 2);
/** The page split before each heading (`hashes`: which heading levels to split on). */
const sections = (md, hashes = '#{1,6}') => md.split(new RegExp(`^(?=${hashes} )`, 'm'));

/** The Model Map's "Video Generation" tables (header row: Model | <id> | <id> …). */
function videoTables(md) {
  const section = sections(md, '##').find((s) => /^## .*Video Generation/.test(s));
  if (!section) throw new Error('Model Map has no "Video Generation" section');
  return tables(section).filter((t) => t[0][0] === 'Model');
}

/** Request Body rows of an endpoint page, as { column header: cell } objects. */
function requestBody(md) {
  const rows = sections(md)
    .filter((s) => /^#{1,6} .*Request Body/.test(s))
    .flatMap((s) => {
      const [header, , ...body] = tables(s)[0] ?? [];
      return header ? body.map((r) => Object.fromEntries(header.map((h, k) => [h, r[k] ?? '']))) : [];
    });
  if (rows.length === 0) throw new Error('no Request Body table');
  return rows;
}

/** Values listed after "Accepted values:" in a `model` row, before the per-model notes. */
function acceptedModels(row) {
  const list = row.Description.match(/Accepted values:(.*?)(?:<br>\s*-|$)/i);
  if (!list) throw new Error('`model` row has no "Accepted values:" list');
  return list[1].match(API_ID) ?? [];
}

const modelRows = (md) => {
  const rows = requestBody(md).filter((r) => r.Field === 'model');
  if (rows.length === 0) throw new Error('Request Body has no `model` row');
  return rows;
};

const endpoint = (key, slug) => ({
  key,
  url: `${DOCS}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => modelRows(body).flatMap(acceptedModels),
  // Field, sub-field, type, required, and the accepted models / code-formatted
  // values and defaults of each request field — the Description prose churns.
  snapshot: (_md, body) =>
    requestBody(body)
      .map((r) => {
        const values =
          r.Field === 'model'
            ? acceptedModels(r)
            : [...r.Description.matchAll(/`([^`]+)`/g)].map((m) => m[1].replace(/<br\s*\/?>/g, ' ').trim()).filter((v) => /[a-z0-9]/i.test(v));
        return [r.Field, r['Sub Field'], r.Type, r.Required, values.join(' ')].filter((c) => c !== undefined).join(' | ');
      })
      .join('\n'),
});

export default {
  provider: 'vidu',
  models: {
    'vidu/viduq1': 'viduq1',
    'vidu/vidu2.0': 'vidu2.0',
    'vidu/viduq2-pro': 'viduq2-pro',
  },
  sources: [
    {
      key: 'model-map',
      url: `${DOCS}/model-map.md`,
      expect: 'text',
      extract: (_md, body) => videoTables(body).flatMap((t) => t[0].slice(1)),
      // Every row but the prose Description: resolution, frame rate, duration, and which endpoints each model supports.
      snapshot: (_md, body) =>
        videoTables(body)
          .map((t) => t.filter((r, k) => k !== 1 && r[0] !== 'Description').map((r) => r.join(' | ')).join('\n'))
          .join('\n\n'),
    },
    endpoint('text-to-video', 'text-to-video'),
    endpoint('image-to-video', 'image-to-video'),
    endpoint('start-end-to-video', 'start-end-to-video'),
    endpoint('reference-to-video', 'reference-to-video'),
    {
      key: 'pages',
      index: true,
      url: `${DOCS}/llms.txt`,
      expect: 'text',
      extract: /\/docs\/[\w-]+\.md/g,
    },
  ],
  acknowledged: [],
};
