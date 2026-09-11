// Kling — the official API docs (app.klingai.com/…/document-api) are a JS app,
// but every page has an LLM markdown twin at kling.ai/document-api/<page>.md,
// indexed by app.klingai.com/global/dev/document-api/llms.txt. Each API page's
// "Create Task" section carries a Request Body table whose `model_name` row
// lists the enum the endpoint accepts — those enums are the ids we compare.
//   images       POST /v1/images/generations (the Image 2.1 page — every
//                image page documents the same table)
//   text2video   POST /v1/videos/text2video (the Kling 1.6 page)
//   image2video  POST /v1/videos/image2video (the Kling 2.1 page)
//   image-2-0, image-1-5  index: our image models' own pages, snapshotted for
//                their retirement notice (the video pages above are our video
//                models' own pages, so their snapshots carry it too). All four
//                pages say "will be retired on September 15, 2026"; if Kling
//                deletes them afterwards these sources 404, which is the cue to
//                retire the models here and in models/kling.yaml
//   pages        index: the video/image API pages in llms.txt — new model
//                families (e.g. the new `/text-to-video/kling-3.0` API) show up here
// Mapping mirrors litegen-core/src/providers/image/kling.rs and
// video/kling.rs `resolve_model` (strip `kling/` and `video-`; sent as body
// `model_name`). Kling reuses version ids across endpoints (kling-v2-1 is both
// Kling Image 2.1 and Kling 2.1 video), so an uncarried model that shares an
// id with a carried one on another endpoint cannot surface as new.

const DOCS = 'https://kling.ai/document-api';

const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());
const backticked = (s) => [...s.matchAll(/`([^`]+)`/g)].map((m) => m[1]);

/** Lines of the "## Create Task" section, after checking it documents `method path`. */
function createTask(md, endpoint) {
  const lines = md.split('\n');
  const start = lines.findIndex((l) => l.trim() === '## Create Task');
  if (start < 0) throw new Error('no "## Create Task" section — the page was restructured');
  const end = lines.findIndex((l, i) => i > start && /^## /.test(l));
  const section = lines.slice(start + 1, end < 0 ? undefined : end);
  const [method, path] = endpoint.split(' ');
  const documented = `${section.find((l) => /^- Method:/.test(l))?.match(/`([^`]+)`/)?.[1]} ${section.find((l) => /^- Path:/.test(l))?.match(/`([^`]+)`/)?.[1]}`;
  if (documented !== `${method} ${path}`) throw new Error(`page documents ${documented}, expected ${endpoint} — the page moved to another API`);
  return section;
}

/** The Request Body table rows of the Create Task section. */
function requestBody(section) {
  const start = section.findIndex((l) => l.trim() === '### Request Body');
  if (start < 0) throw new Error('no "### Request Body" table — the page was restructured');
  const rows = [];
  for (const l of section.slice(start + 1)) {
    if (/^#/.test(l)) break;
    if (l.startsWith('|')) rows.push(cells(l));
  }
  if (rows.length < 3) throw new Error('empty Request Body table — the page was restructured');
  return rows;
}

const modelNames = (section) => {
  const row = requestBody(section).find((c) => c[0] === '`model_name`');
  if (!row) throw new Error('no `model_name` row in the Request Body table');
  return backticked(row[4] ?? '');
};

/** Retirement / deprecation notices (blockquotes) above the first section. */
const notices = (md) => {
  const head = md.split('\n## ')[0];
  const found = head.split('\n').filter((l) => /^>\s/.test(l) && /retire|deprecat|offline|discontinu/i.test(l));
  return found.length ? found.map((l) => l.trim()).join('\n') : '(no retirement notice)';
};

/** Retirement notice + the Request Body table without its prose Description column. */
const typings = (endpoint) => (md) =>
  `# notice\n${notices(md)}\n\n# ${endpoint} request body\n` +
  requestBody(createTask(md, endpoint))
    .map((c) => `| ${c.slice(0, 5).join(' | ')} |`)
    .join('\n') +
  '\n';

const apiPage = (key, page, endpoint) => ({
  key,
  url: `${DOCS}/api/${page}.md`,
  expect: 'text',
  extract: (md) => modelNames(createTask(md, endpoint)),
  snapshot: typings(endpoint),
});

const lifecyclePage = (key, page, endpoint) => ({
  key,
  index: true,
  url: `${DOCS}/api/${page}.md`,
  expect: 'text',
  extract: (md) => modelNames(createTask(md, endpoint)),
  snapshot: (md) => `${notices(md)}\n`,
});

export default {
  provider: 'kling',
  models: {
    'kling/kling-v2': 'kling-v2',
    'kling/kling-v1-5': 'kling-v1-5',
    // Image 2.1 and Video 2.1 share the vendor id kling-v2-1 (different endpoints).
    'kling/kling-v2-1': 'kling-v2-1',
    'kling/video-kling-v2-1': 'kling-v2-1',
    'kling/video-kling-v1-6': 'kling-v1-6',
    'kling/video-kling-v2-5-turbo': 'kling-v2-5-turbo',
    'kling/video-kling-v2-6': 'kling-v2-6',
  },
  sources: [
    apiPage('images', 'image/2-1/image-generation', 'POST /v1/images/generations'),
    apiPage('text2video', 'video/1-6/text-to-video', 'POST /v1/videos/text2video'),
    apiPage('image2video', 'video/2-1/image-to-video', 'POST /v1/videos/image2video'),
    lifecyclePage('image-2-0', 'image/2-0/image-generation', 'POST /v1/images/generations'),
    lifecyclePage('image-1-5', 'image/1-5/image-generation', 'POST /v1/images/generations'),
    {
      key: 'pages',
      index: true,
      url: 'https://app.klingai.com/global/dev/document-api/llms.txt',
      expect: 'text',
      extract: /\/document-api\/api\/((?:video|image)\/[\w./-]+?)\.md/g,
    },
  ],
  // kling-v3 is left unacknowledged on purpose (add later).
  acknowledged: [
    { id: 'kling-v1', reason: 'skipped 2026-09-11: retires upstream 2026-09-15' },
    { id: 'kling-v2-master', reason: 'skipped 2026-09-11: retires upstream 2026-09-15' },
    { id: 'kling-v2-1-master', reason: 'skipped 2026-09-11: retires upstream 2026-09-15' },
    { id: 'kling-v2-new', reason: 'skipped 2026-09-11: retires upstream 2026-09-15' },
  ],
};
