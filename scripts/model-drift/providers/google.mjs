// Google (Gemini API) — the docs site's markdown twins (`<page>.md.txt`) plus
// the API's public discovery document; the HTML pages are large and the
// list-models API needs a key.
//   models           the "Generative media models" table of the models overview
//                    (every endpoint id in that section: the section is the scope,
//                    so a new media family — e.g. `gemini-nano-banana-*` — cannot
//                    slip past a name filter)
//   veo              the Veo guide: model codes, the request-parameter table, the
//                    request keys its REST samples send and the per-model limits
//                    (it documents the `:predictLongRunning` surface we call)
//   image-generation the generateContent image guide ("Nano Banana"): the
//                    aspect-ratio × size tables (### Aspect ratios and image size,
//                    up to the next ## heading), the reference-image limits table
//                    and the request keys its REST samples send
//   discovery        index: the v1beta discovery document — the methods our
//                    adapters call (generateContent, predictLongRunning, predict)
//                    and the request schemas they fill (GenerationConfig,
//                    ImageConfig, ResponseFormatConfig.image, Part/Blob/FileData);
//                    Veo's instances/parameters are untyped there (`any`), so the
//                    veo guide stays the evidence for those
//   imagen           the Imagen model card — the overview abbreviates the
//                    family to `imagen-4.0-generate`, the card has real ids.
//                    Imagen 4 is shut down (overview: "Imagen 4 (Shut down)");
//                    when Google deletes the card this source 404s — drop it then
//   lifecycle        index: the deprecations table rows for our models, so a
//                    newly announced shutdown date shows up as a changed snapshot
//   pages            index: image/video docs pages listed in llms.txt
// Mapping mirrors litegen-core/src/providers/image/google.rs `resolve_model`
// and video/google.rs `resolve_model` (strip `google/`; the id is the
// `models/{id}:generateContent` / `:predictLongRunning` path segment). The
// image adapter's Imagen `:predict` branch is unreachable for carried models
// (`resolve_model` never yields an `imagen-*` id).

const DOCS = 'https://ai.google.dev/gemini-api/docs';
const DISCOVERY = 'https://generativelanguage.googleapis.com/$discovery/rest?version=v1beta';

// Image/video generation ids only — the deprecations page lists many text/audio LLMs.
const MEDIA_ID = /^(?:gemini-[\w.-]*(?:image|nano-banana)[\w.-]*|gemini-omni[\w.-]*|imagen-[\w.-]+|veo-[\w.-]+)$/;

/**
 * Lines after the `from` heading line up to (not including) the `to` line
 * (default: the next heading at the same or higher level). `from` / `to` match
 * a trimmed line exactly (string) or by pattern (RegExp).
 */
function section(md, from, to) {
  const lines = md.split('\n');
  const is = (l, m) => (m instanceof RegExp ? m.test(l.trim()) : l.trim() === m);
  const start = lines.findIndex((l) => is(l, from));
  if (start < 0) throw new Error(`section "${from}" not found — the page was restructured`);
  const level = lines[start].trim().match(/^#+/)[0].length;
  const end = lines.findIndex((l, i) =>
    i > start && (to ? is(l, to) : /^#+ /.test(l) && l.match(/^#+/)[0].length <= level),
  );
  if (to && end < 0) throw new Error(`section end "${to}" not found — the page was restructured`);
  return lines.slice(start + 1, end < 0 ? undefined : end);
}

const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());
const backticked = (s) => [...s.matchAll(/`([^`\s]+)`/g)].map((m) => m[1]);

/** Ids in the "Model code" rows (`| Model code | **Gemini API** `a` `b` |`) of a section. */
const modelCodes = (lines) => lines.filter((l) => /^\|\s*Model code\s*\|/.test(l)).flatMap(backticked);

/**
 * The image-generation page's "Model selection" bullets — `**Nickname ([Model
 * name](…)) (`model-id`):**` — as `model-id — Nickname / Model name`. Its
 * tables are headed by those names only, so this is what ties them to an id.
 */
function modelSelection(md) {
  const text = section(md, '## Model selection').join(' ');
  const rows = [...text.matchAll(/\*\*([^*(]+?)\s*\(\[([^\]]+)\]\([^)]*\)\)\s*\(`([^`]+)`\):\*\*/g)].map(
    ([, nickname, name, id]) => `${id} — ${nickname.trim()} / ${name.trim()}`,
  );
  if (rows.length === 0) throw new Error('"## Model selection" lists no `model-id` bullets — the page was restructured');
  return rows.join('\n');
}

/** Tables in a line range, each preceded by the heading it sits under. */
function tablesWithHeadings(lines) {
  const out = [];
  let heading = null;
  for (const l of lines) {
    if (/^#+ /.test(l)) heading = l.trim();
    else if (l.startsWith('|')) {
      if (heading) out.push('', heading);
      heading = null;
      out.push(l.trim());
    }
  }
  if (out.length === 0) throw new Error('no tables in the section — the page was restructured');
  return out.join('\n').trim() + '\n';
}

/** JSON keys the page's REST samples send (sorted, unique): a new or renamed request field changes this list. */
function restKeys(md) {
  const keys = new Set();
  let inRest = false;
  for (const l of md.split('\n')) {
    if (/^#+ /.test(l)) inRest = /^#+ REST\s*$/.test(l);
    else if (inRest) for (const m of l.matchAll(/\\?"([A-Za-z_]\w*)\\?"\s*:/g)) keys.add(m[1]);
  }
  if (keys.size === 0) throw new Error('no REST samples found — the page was restructured');
  return [...keys].sort().join('\n') + '\n';
}

// ─── Discovery document ──────────────────────────────────────────────────────

// models.* methods the adapters call. predict is Imagen-only and unreachable
// for carried models, so its absence would not fail the source.
const CALLED = ['generateContent', 'predictLongRunning'];
const LISTED = [...CALLED, 'predict'];

// Request schemas the adapters fill → the properties to keep (null: all).
// `prose: true` keeps descriptions where they carry the allowed values
// (ImageConfig's aspectRatio / imageSize are plain strings).
const SCHEMAS = {
  GenerateContentRequest: { keep: ['contents', 'generationConfig'] },
  Content: { keep: ['parts'] },
  Part: { keep: ['text', 'inlineData', 'fileData'] },
  Blob: {},
  FileData: {},
  GenerationConfig: {},
  ImageConfig: { prose: true },
  ResponseFormatConfig: { keep: ['image'] },
  ImageResponseFormat: {},
  PredictLongRunningRequest: {},
  PredictRequest: {},
};

/** One property's typing: type / $ref / items / enum / format / deprecated (+ description when asked). */
function typing(p, prose) {
  const out = {};
  for (const k of ['type', '$ref', 'format', 'enum', 'enumDeprecated', 'deprecated']) if (p[k] !== undefined) out[k] = p[k];
  if (p.items) out.items = typing(p.items, false);
  if (prose && p.description) out.description = p.description.replace(/\s+/g, ' ').trim();
  return out;
}

function discoveryExcerpt(doc) {
  const methods = doc.resources?.models?.methods ?? {};
  const lines = ['# models.* methods the adapters call'];
  for (const m of LISTED) {
    const x = methods[m];
    lines.push(x ? `${m}: ${x.httpMethod} ${x.path} ← ${x.request?.$ref ?? '(no body)'}` : `${m}: (absent)`);
  }
  const op = doc.resources?.models?.resources?.operations?.methods?.get;
  lines.push(`operations.get: ${op ? `${op.httpMethod} ${op.path}` : '(absent)'}`, '', '# request schemas');
  const out = {};
  for (const [name, { keep, prose }] of Object.entries(SCHEMAS)) {
    const s = doc.schemas?.[name];
    if (!s) throw new Error(`schema ${name} is not in the discovery document — renamed or removed upstream`);
    const props = s.properties ?? {};
    for (const k of keep ?? []) if (!props[k]) throw new Error(`${name}.${k} is not in the discovery document — renamed or removed upstream`);
    const names = (keep ?? Object.keys(props)).slice().sort();
    out[name] = Object.fromEntries(names.map((k) => [k, typing(props[k], prose)]));
  }
  return `${lines.join('\n')}\n${JSON.stringify(out, null, 2)}\n`;
}

export default {
  provider: 'google',
  models: {
    'google/gemini-3.1-flash-image': 'gemini-3.1-flash-image',
    'google/gemini-3.1-flash-lite-image': 'gemini-3.1-flash-lite-image',
    'google/gemini-2.5-flash-image': 'gemini-2.5-flash-image',
    'google/gemini-3-pro-image': 'gemini-3-pro-image',
    'google/veo-3.1-generate-preview': 'veo-3.1-generate-preview',
    'google/veo-3.1-fast-generate-preview': 'veo-3.1-fast-generate-preview',
    'google/veo-3.1-lite-generate-preview': 'veo-3.1-lite-generate-preview',
  },
  sources: [
    {
      key: 'models',
      url: `${DOCS}/models.md.txt`,
      expect: 'text',
      // Endpoint cells are ``` id ``` (a cell may hold several ids).
      extract: (md) =>
        section(md, '## Generative media models')
          .filter((l) => l.startsWith('|'))
          .flatMap((l) => [...l.matchAll(/```([^`]+)```/g)].flatMap((m) => m[1].trim().split(/\s+/))),
    },
    {
      key: 'veo',
      url: `${DOCS}/veo.md.txt`,
      expect: 'text',
      extract: (md) => modelCodes(section(md, '## Model versions')),
      snapshot: (md) =>
        // The parameter table, first column cut to the parameter name (its prose varies).
        `${section(md, '## Veo API parameters and specifications')
          .filter((l) => l.startsWith('|'))
          .map((l) => {
            const [first, ...rest] = cells(l);
            return `| ${[backticked(first)[0] ?? first, ...rest].join(' | ')} |`;
          })
          .join('\n')}\n\n` +
        // Per-model input/output limits (prompt token cap), without the churning "Latest update" row.
        `# model versions\n${tablesWithHeadings(
          section(md, '## Model versions').filter((l) => !/^\|\s*Latest update\s*\|/.test(l)),
        )}\n` +
        `# request keys in the REST samples\n${restKeys(md)}`,
    },
    {
      key: 'image-generation',
      url: `${DOCS}/generate-content/image-generation.md.txt`,
      expect: 'text',
      // A version number after `gemini-` skips image file names like gemini-native-image.png;
      // `gemini-nano-banana-<n>` is the naming Google adopted with Nano Banana 2.1.
      extract: /\bgemini-(?:[\d.]+-[a-z0-9-]*image[a-z0-9-]*|nano-banana(?:-[a-z0-9]+(?:\.\d+)*)*)(?!\.(?:png|jpe?g|gif|webp|svg))/g,
      snapshot: (md) =>
        `# models the page documents (the tables below are headed by their names)\n${modelSelection(md)}\n` +
        `# aspect ratio x size tables\n${tablesWithHeadings(section(md, '### Aspect ratios and image size', /^## /))}\n` +
        `# reference image limits\n${tablesWithHeadings(section(md, /^### Use up to \d+ reference images$/))}\n` +
        `# request keys in the REST samples\n${restKeys(md)}`,
    },
    {
      key: 'discovery',
      index: true,
      url: DISCOVERY,
      expect: 'json',
      extract: (doc) => {
        const methods = doc.resources?.models?.methods ?? {};
        const gone = CALLED.filter((m) => !methods[m]);
        if (gone.length) throw new Error(`models.${gone.join(', models.')} not in the discovery document — renamed or removed upstream`);
        return LISTED.filter((m) => methods[m]);
      },
      snapshot: (doc) => discoveryExcerpt(doc),
    },
    {
      key: 'imagen',
      url: `${DOCS}/models/imagen.md.txt`,
      expect: 'text',
      extract: (md) => modelCodes(md.split('\n')),
    },
    {
      key: 'lifecycle',
      index: true,
      url: `${DOCS}/deprecations.md.txt`,
      expect: 'text',
      extract: (md) =>
        md.split('\n').filter((l) => l.startsWith('|')).map((l) => backticked(cells(l)[0] ?? '')[0]).filter((id) => id && MEDIA_ID.test(id)),
      // Only our models' rows: a new model surfaces as a new id elsewhere, not as churn here.
      snapshot: (md, _body, { ours }) => {
        const want = new Set(ours.map((s) => s.toLowerCase()));
        const rows = md
          .split('\n')
          .filter((l) => l.startsWith('|') && want.has((backticked(cells(l)[0] ?? '')[0] ?? '').toLowerCase()))
          .map((l) => `| ${cells(l).join(' | ')} |`)
          .sort();
        if (rows.length === 0) throw new Error('none of our models are in the deprecations tables — the page was restructured');
        return `| Model | Release date | Shutdown date | Recommended replacement |\n${rows.join('\n')}\n`;
      },
    },
    {
      key: 'pages',
      index: true,
      url: `${DOCS}/llms.txt`,
      expect: 'text',
      extract: (md) =>
        [...md.matchAll(/\/gemini-api\/docs\/([\w./-]*(?:image|imagen|video|veo|omni|banana)[\w.-]*)\.md\.txt/g)]
          .map((m) => m[1])
          .filter((p) => !/understanding|robotics/.test(p)),
    },
  ],
  acknowledged: [
    {
      id: 'imagen-4.0-generate',
      reason: 'the models overview abbreviates the Imagen 4 family; the imagen source carries the real ids (imagen-4.0-*generate-001)',
    },
    // Deprecated upstream; Google's migration targets are carried (gemini-3.1-flash-image, veo-3.1-*).
    // @see https://ai.google.dev/gemini-api/docs/deprecations
    // gemini-omni-1.1-flash stays unacknowledged on purpose: add later (it needs the Interactions API).
    { id: 'imagen-4.0-generate-001', reason: 'skipped 2026-09-11: Imagen 4 is deprecated and its announced shutdown (2026-08-17) has passed' },
    { id: 'imagen-4.0-ultra-generate-001', reason: 'skipped 2026-09-11: Imagen 4 is deprecated and its announced shutdown (2026-08-17) has passed' },
    { id: 'imagen-4.0-fast-generate-001', reason: 'skipped 2026-09-11: Imagen 4 is deprecated and its announced shutdown (2026-08-17) has passed' },
    { id: 'veo-3.0-generate-001', reason: 'skipped 2026-09-11: Veo 3.0 is deprecated (earliest shutdown 2026-06-30); the veo-3.1 line is carried' },
    { id: 'veo-3.0-fast-generate-001', reason: 'skipped 2026-09-11: Veo 3.0 is deprecated (earliest shutdown 2026-06-30); the veo-3.1 line is carried' },
  ],
};
