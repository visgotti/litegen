// Vidu — the docs site serves a markdown twin of every page (`<page>.md`,
// indexed by llms.txt). Sources: the Model Map's video tables (the lineup, with
// resolution/duration per model), and the endpoint pages for the four endpoints
// our adapter routes between. Since 2026-10 the API reference has one page per
// model family and endpoint (`video-models/<family>/<endpoint>.md`), holding one
// operation (a fenced `POST /ent/v2/<endpoint>` block and its body table) per
// model. Ids come from table structure only: the Model Map's header rows, and
// each operation's `model` row (its code-formatted values and `<id>:` value
// definitions) plus the `"model"` of its request example — so marketing names
// in prose and headings ("ViduQ3 Series") are not counted.
// Mapping mirrors litegen-core/src/providers/video/vidu.rs (strip `vidu/`).

const DOCS = 'https://platform.vidu.com/docs';
const ID = /^vidu[a-z0-9][\w.-]*$/i;
// A value definition in a `model` row: "viduq3-turbo: Faster generation …" / "`viduq3-mix`: …".
const DEFINED_ID = /(?<![\w.-])`?(vidu[a-z0-9][\w.-]*?)`?:(?=\s)/gi;

const cells = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());
const tables = (md) =>
  md
    .split(/\n(?!\|)/)
    .map((chunk) => chunk.split('\n').filter((l) => l.startsWith('|')).map(cells))
    .filter((t) => t.length >= 2);
/** The page split before each heading (`hashes`: which heading levels to split on). */
const sections = (md, hashes = '#{1,6}') => md.split(new RegExp(`^(?=${hashes} )`, 'm'));
const codeSpans = (s) => [...s.matchAll(/`([^`]+)`/g)].map((m) => m[1].trim());
const unformat = (s) => s.replace(/`/g, '').trim();

/** The Model Map's "Video Generation" tables (header row: Model | <id> | <id> …). */
function videoTables(md) {
  const section = sections(md, '##').find((s) => /^## .*Video Generation/.test(s));
  if (!section) throw new Error('Model Map has no "Video Generation" section');
  return tables(section).filter((t) => t[0][0] === 'Model');
}

/**
 * The page's operations: each fenced `POST|GET <url>` block, up to the next
 * one. Only the POSTs to `/ent/v2/<endpoint>` — the endpoint this page is
 * for — are kept, each with its request-body tables and accepted model ids.
 */
function operations(md, endpoint) {
  const path = `/ent/v2/${endpoint}`;
  const ops = md
    .split(/^(?=```[^\n]*\n(?:POST|GET) )/m)
    .slice(1)
    .map((chunk) => {
      const [, method, url] = chunk.match(/^```[^\n]*\n(POST|GET) (\S+)/);
      return { method, path: url.replace(/^https?:\/\/[^/]+/, ''), chunk };
    })
    .filter((op) => op.method === 'POST' && op.path === path)
    .map(({ path, chunk }) => {
      const bodies = requestBodies(chunk, path);
      return { path, bodies, models: acceptedModels(chunk, bodies, path) };
    });
  if (ops.length === 0) throw new Error(`no POST ${path} operation on the page`);
  return ops;
}

/**
 * An operation's request-body tables ("Body parameters", or the older "Request
 * Body" a few pages still carry for a second request form), as rows of
 * { name, type, required, description }. A `↳ child` row is named `parent.child`.
 */
function requestBodies(chunk, path) {
  const bodies = sections(chunk)
    .map((s) => ({ heading: s.match(/^#{1,6} (Body parameters|Request Body)\s*\n/i)?.[1], table: tables(s)[0] }))
    .filter((b) => b.heading && b.table)
    .map(({ heading, table }) => {
      const [header, , ...rows] = table;
      const col = Object.fromEntries(['Name', 'Type', 'Required', 'Description'].map((h) => [h, header.indexOf(h)]));
      if (Object.values(col).some((k) => k < 0)) throw new Error(`POST ${path} ${heading} table has no Name | Type | Required | Description header`);
      let parent = '';
      return {
        heading,
        rows: rows.map((r) => {
          const child = /^↳/.test(r[col.Name]);
          const name = unformat(r[col.Name].replace(/^↳\s*/, ''));
          if (!child) parent = name;
          return { name: child ? `${parent}.${name}` : name, type: unformat(r[col.Type] ?? ''), required: r[col.Required] ?? '', description: r[col.Description] ?? '' };
        }),
      };
    });
  if (bodies.length === 0) throw new Error(`POST ${path} has no Body parameters table`);
  return bodies;
}

/** The ids in an operation's `model` rows, plus the `"model"` its request example sends. */
function acceptedModels(chunk, bodies, path) {
  const rows = bodies.flatMap((b) => b.rows).filter((r) => r.name === 'model');
  if (rows.length === 0) throw new Error(`POST ${path} body has no \`model\` row`);
  const listed = rows.flatMap((r) => [...codeSpans(r.description).filter((s) => ID.test(s)), ...[...r.description.matchAll(DEFINED_ID)].map((m) => m[1])]);
  if (listed.length === 0) throw new Error(`POST ${path} \`model\` row lists no model id`);
  const example = [...chunk.matchAll(/^```[^\n]*\n(curl[\s\S]*?)^```/gm)].map((m) => m[1].match(/"model"\s*:\s*"([^"]+)"/)?.[1]).filter(Boolean);
  return [...new Set([...listed, ...example].map((s) => s.toLowerCase()))];
}

/**
 * The literal values in a field description: single-word code spans (enum
 * values) and number-bearing tokens (defaults, bounds, ranges, resolutions,
 * aspect ratios, length limits). The words around them are prose and churn.
 */
function literals(description) {
  const text = description
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/\*\*/g, '')
    .replace(/\bNote\s*\d+\s*[:：]?/gi, ' ');
  const token = /`([A-Za-z_][\w.-]*)`|(?<![\w.:-])(\d+(?:[.,]\d+)*(?:\s*[-–~]\s*\d+)?(?::\d+)?(?:\s*(?:fps|p|k|s)\b)?)/gi;
  return [...text.matchAll(token)].map((m) => (m[1] ?? m[2]).replace(/\s*[-–~]\s*/g, '-').replace(/\s+/g, ''));
}

const ENDPOINTS = {
  'text-to-video': 'text2video',
  'image-to-video': 'img2video',
  'start-end-to-video': 'start-end2video',
  'reference-to-video': 'reference2video',
};

/** One endpoint page of one model family (`vidu-q3`) — keyed `q3-image-to-video`. */
const endpoint = (family, slug) => ({
  key: `${family.replace(/^vidu-/, '')}-${slug}`,
  url: `${DOCS}/api-reference/video-models/${family}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => operations(body, ENDPOINTS[slug]).flatMap((op) => op.models),
  // Per operation: the endpoint and the models it accepts; then, for the
  // operations of models we carry, each body field's name, type, required and
  // literal values. Other models' operations stay one line, so their edits
  // don't churn this baseline (a new model surfaces as a new id instead).
  snapshot: (_md, body, { ours }) => {
    const carried = new Set(ours.map((s) => s.toLowerCase()));
    return operations(body, ENDPOINTS[slug])
      .map((op) => {
        const head = `POST ${op.path} · ${op.models.join(' ')}`;
        if (!op.models.some((m) => carried.has(m))) return `${head} (not carried)`;
        const tables = op.bodies.map((b) =>
          [b.heading, ...b.rows.map((r) => [r.name, r.type, r.required, (r.name === 'model' ? op.models : literals(r.description)).join(' ')].join(' | '))].join('\n'),
        );
        return [head, ...tables].join('\n');
      })
      .join('\n\n');
  },
});

// The families whose pages exist for each endpoint (vidu-q4-preview has no
// text-to-video or start-end-to-video page). vidu2.0 has no endpoint page at
// all any more; only the Model Map lists it. A new family or page shows up as
// a changed `pages` snapshot.
const FAMILY_PAGES = {
  'vidu-q4-preview': ['image-to-video', 'reference-to-video'],
  'vidu-q3': Object.keys(ENDPOINTS),
  'vidu-q2': Object.keys(ENDPOINTS),
  'vidu-q1': Object.keys(ENDPOINTS),
};

export default {
  provider: 'vidu',
  models: {
    'vidu/viduq1': 'viduq1',
    'vidu/vidu2.0': 'vidu2.0',
    'vidu/viduq2-pro': 'viduq2-pro',
    'vidu/viduq3-turbo': 'viduq3-turbo',
    'vidu/viduq3-pro': 'viduq3-pro',
    'vidu/viduq2-pro-fast': 'viduq2-pro-fast',
  },
  sources: [
    {
      key: 'model-map',
      url: `${DOCS}/overview/model-map.md`,
      expect: 'text',
      extract: (_md, body) => videoTables(body).flatMap((t) => t[0].slice(1)),
      // Every row but the prose one (Description, renamed Introduction in
      // 2026-10): resolution, frame rate, duration, and which endpoints each
      // model supports.
      snapshot: (_md, body) =>
        videoTables(body)
          .map((t) => t.filter((r, k) => k !== 1 && !/^(Description|Introduction)$/i.test(r[0])).map((r) => r.join(' | ')).join('\n'))
          .join('\n\n'),
    },
    ...Object.entries(FAMILY_PAGES).flatMap(([family, slugs]) => slugs.map((slug) => endpoint(family, slug))),
    {
      key: 'pages',
      index: true,
      url: `${DOCS}/llms.txt`,
      expect: 'text',
      // Markdown link targets on the docs host: "- [Model Map](https://platform.vidu.com/docs/overview/model-map.md): …"
      extract: /\]\(https:\/\/platform\.vidu\.com(\/docs\/[\w./-]+\.md)\)/g,
    },
  ],
  acknowledged: [
    // Skipped 2026-09-11 when only the Model Map listed them; since the 2026-10
    // docs restructure both have their own reference2video operation
    // (video-models/vidu-q3/reference-to-video.md), so the premise of the skip
    // no longer holds — re-decide.
    { id: 'viduq3-drama', reason: 'skipped 2026-09-11: then in the Model Map only (now documented on reference2video)' },
    { id: 'viduq3-ad', reason: 'skipped 2026-09-11: then in the Model Map only (now documented on reference2video)' },
    { id: 'viduq3', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
    { id: 'viduq3-mix', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
    { id: 'viduq3-pro-fast', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
    { id: 'viduq2', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
    { id: 'viduq2-turbo', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
    { id: 'viduq1-classic', reason: 'skipped 2026-09-11: optional, not carried; revisit on demand' },
  ],
};
