// Google (Gemini API) — the docs site's markdown twins (`<page>.md.txt`); the
// HTML pages are large and the list-models API needs a key.
//   models           the "Generative media models" table of the models overview
//   veo              the Veo guide: model codes + the request-parameter table
//                    (it documents the `:predictLongRunning` surface we call)
//   image-generation the generateContent flavour of the image guide (the docs
//                    default to the Interactions API; our adapter calls
//                    `:generateContent`): aspect-ratio × size tables + the
//                    request keys its REST samples send
//   imagen           the Imagen model card — the overview abbreviates the
//                    family to `imagen-4.0-generate`, the card has real ids.
//                    Imagen 4 is deprecated (earliest shutdown 2026-08-17, not
//                    yet marked shut down); when Google deletes the card this
//                    source 404s — drop it then
//   lifecycle        index: the deprecations table rows for our models, so a
//                    newly announced shutdown date shows up as a changed snapshot
//   pages            index: image/video docs pages listed in llms.txt
// Mapping mirrors litegen-core/src/providers/image/google.rs `resolve_model`
// and video/google.rs `resolve_model` (strip `google/`; the id is the
// `models/{id}:generateContent` / `:predictLongRunning` path segment).

const DOCS = 'https://ai.google.dev/gemini-api/docs';

// Image/video generation ids only — the same pages list many text/audio LLMs.
const MEDIA_ID = /^(?:gemini-[\w.-]*image[\w-]*|gemini-omni[\w.-]*|imagen-[\w.-]+|veo-[\w.-]+)$/;

/** Lines from the `from` heading line up to (not including) the `to` heading line (default: next heading at the same or higher level). */
function section(md, from, to) {
  const lines = md.split('\n');
  const start = lines.findIndex((l) => l.trim() === from);
  if (start < 0) throw new Error(`section "${from}" not found — the page was restructured`);
  const level = from.match(/^#+/)[0].length;
  const end = lines.findIndex((l, i) =>
    i > start && (to ? l.trim() === to : /^#+ /.test(l) && l.match(/^#+/)[0].length <= level),
  );
  if (to && end < 0) throw new Error(`section end "${to}" not found — the page was restructured`);
  return lines.slice(start + 1, end < 0 ? undefined : end);
}

const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());
const backticked = (s) => [...s.matchAll(/`([^`\s]+)`/g)].map((m) => m[1]);

/** Ids in the "Model code" rows (`| Model code | **Gemini API** `a` `b` |`) of a section. */
const modelCodes = (lines) => lines.filter((l) => /^\|\s*Model code\s*\|/.test(l)).flatMap(backticked);

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
          .flatMap((l) => [...l.matchAll(/```([^`]+)```/g)].flatMap((m) => m[1].trim().split(/\s+/)))
          .filter((id) => MEDIA_ID.test(id)),
    },
    {
      key: 'veo',
      url: `${DOCS}/veo.md.txt`,
      expect: 'text',
      extract: (md) => modelCodes(section(md, '## Model versions')),
      // The parameter table, first column cut to the parameter name (its prose varies).
      snapshot: (md) =>
        section(md, '## Veo API parameters and specifications')
          .filter((l) => l.startsWith('|'))
          .map((l) => {
            const [first, ...rest] = cells(l);
            return `| ${[backticked(first)[0] ?? first, ...rest].join(' | ')} |`;
          })
          .join('\n') + '\n',
    },
    {
      key: 'image-generation',
      url: `${DOCS}/generate-content/image-generation.md.txt`,
      expect: 'text',
      // A version number after `gemini-` skips image file names like gemini-native-image.png.
      extract: /\bgemini-[\d.]+-[a-z0-9-]*image[a-z0-9-]*(?!\.(?:png|jpe?g|gif|webp|svg))/g,
      snapshot: (md) =>
        `# aspect ratio x size tables\n${tablesWithHeadings(section(md, '### Aspect ratios and image size', '## Model selection'))}\n` +
        `# request keys in the REST samples\n${restKeys(md)}`,
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
        [...md.matchAll(/\/gemini-api\/docs\/([\w./-]*(?:image|imagen|video|veo|omni)[\w.-]*)\.md\.txt/g)]
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
