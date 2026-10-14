// Kling — the official API docs (app.klingai.com/…/document-api) are a JS app,
// but every page has an LLM markdown twin at kling.ai/document-api/<page>.md,
// indexed by app.klingai.com/global/dev/document-api/llms.txt. Each API page's
// "## Create Task" section names its endpoint (`- Method:` / `- Path:`) and
// carries a Request Body table plus "Request Body Field Notes", which hold the
// per-model limits the table cannot (prompt length, "Only kling-v2-1 supports
// this parameter", "first + last frame only supports 1080p").
//
// Kling runs two API designs side by side (get-started/authentication.md; API
// Updates 2026-07-15: "The legacy API will continue to be available with no
// current plans for deprecation"):
//   legacy  POST /v1/<endpoint>, model in the body as `model_name`, AK/SK JWT.
//           Both adapters speak this design.
//   new     POST /<endpoint>/<model>, model in the path, API key only.
// Images: the legacy /v1/images/generations page is still published, so its
// `model_name` enum is the id list. Video: Kling deleted the legacy
// /v1/videos/text2video and /v1/videos/image2video docs together with the
// Kling 1.6 / 2.1 pages that carried them (both retired 2026-09-15). Every
// video model's page now documents only the new API, so video ids are the new
// API's path models (`/text-to-video/kling-2.6`), and each carried video
// model's typings evidence is its own page's request body.
//   images      POST /v1/images/generations (the Image 2.1 page; the Image 3.0
//               page documents the same endpoint with a byte-identical body)
//   t2v-<ver>, i2v-<ver>  POST /text-to-video/<model>, /image-to-video/<model>:
//               every text/image-to-video page Kling publishes — the family the
//               adapter's text2video / image2video calls belong to — so an
//               uncarried model on it surfaces as `new`
//   capability-video, capability-image  index: the capability maps, which the
//               request bodies defer to for per-model support ("Different model
//               versions support varying ranges") and models/kling.yaml cites
//   pages       index: the video/image API pages in llms.txt — a new model
//               family's page (e.g. a future `video/3-5/text-to-video`) shows up
//               here first; give it a t2v/i2v source
// The old image-2-0 / image-1-5 lifecycle sources watched those pages for their
// 2026-09-15 retirement notice; Kling deleted the pages on retirement, as they
// anticipated. The retired models now surface as `missing` instead.

const DOCS = 'https://kling.ai/document-api';

const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());
const backticked = (s) => [...s.matchAll(/`([^`]+)`/g)].map((m) => m[1]);

/** Lines under `## <heading>` up to the next `## ` heading. */
function section(md, heading) {
  const lines = md.split('\n');
  const start = lines.findIndex((l) => l.trim() === `## ${heading}`);
  if (start < 0) throw new Error(`no "## ${heading}" section — the page was restructured`);
  const end = lines.findIndex((l, i) => i > start && /^## /.test(l));
  return lines.slice(start + 1, end < 0 ? undefined : end);
}

/** The first markdown table in `lines`, rendered without its prose Description column. */
function table(lines) {
  const rows = [];
  const start = lines.findIndex((x) => x.startsWith('|'));
  for (const l of start < 0 ? [] : lines.slice(start)) {
    if (!l.startsWith('|')) break;
    rows.push(cells(l));
  }
  if (rows.length < 3) throw new Error('empty table — the page was restructured');
  const keep = rows[0].map((h, i) => (h === 'Description' ? -1 : i)).filter((i) => i >= 0);
  return rows.map((c) => `| ${keep.map((i) => c[i] ?? '').join(' | ')} |`);
}

/** The "## Create Task" section and the endpoint it documents ("POST /v1/…"). */
function createTask(md) {
  const lines = section(md, 'Create Task');
  const field = (name) => lines.find((l) => l.startsWith(`- ${name}:`))?.match(/`([^`]+)`/)?.[1];
  return { lines, endpoint: `${field('Method')} ${field('Path')}` };
}

/** The Request Body table of the Create Task section. */
function requestBody(task) {
  const start = task.lines.findIndex((l) => l.trim() === '### Request Body');
  if (start < 0) throw new Error('no "### Request Body" table — the page was restructured');
  const end = task.lines.findIndex((l, i) => i > start && /^#/.test(l));
  return task.lines.slice(start + 1, end < 0 ? undefined : end);
}

/** "Request Body Field Notes" bullets (and quoted rules), minus fenced JSON examples. */
function fieldNotes(task) {
  const start = task.lines.findIndex((l) => l.trim() === '#### Request Body Field Notes');
  if (start < 0) return [];
  const out = [];
  let fenced = false;
  for (const l of task.lines.slice(start + 1)) {
    if (/^#{1,4} /.test(l)) break;
    const fences = (l.match(/```/g) ?? []).length;
    if (fenced || fences) {
      if (fences % 2) fenced = !fenced;
      continue;
    }
    const t = l.replace(/\s+$/, '');
    if (/^\s*- /.test(t) || /^\s*>\s*\S/.test(t)) out.push(t);
  }
  return out;
}

/** Retirement / deprecation notices (blockquotes) above the first section. */
const notices = (md) => {
  const head = md.split('\n## ')[0];
  const found = head.split('\n').filter((l) => /^>\s/.test(l) && /retire|deprecat|offline|discontinu/i.test(l));
  return found.length ? found.map((l) => l.trim()).join('\n') : '(no retirement notice)';
};

/** Retirement notice + Request Body table (no Description column) + field notes. */
const typings = (md, task) =>
  [
    `# notice\n${notices(md)}`,
    `# ${task.endpoint} request body\n${table(requestBody(task)).join('\n')}`,
    `# field notes\n${fieldNotes(task).join('\n') || '(none)'}`,
  ].join('\n\n') + '\n';

/** A legacy-design page: ids are the `model_name` enum of `endpoint`'s Request Body. */
const legacyPage = (key, page, endpoint) => {
  const task = (md) => {
    const t = createTask(md);
    if (t.endpoint !== endpoint) throw new Error(`page documents ${t.endpoint}, expected ${endpoint} — the page moved to another API`);
    return t;
  };
  return {
    key,
    url: `${DOCS}/api/${page}.md`,
    expect: 'text',
    extract: (md) => {
      const row = table(requestBody(task(md))).map((l) => cells(l)).find((c) => c[0] === '`model_name`');
      if (!row) throw new Error('no `model_name` row in the Request Body table');
      return backticked(row[4] ?? '');
    },
    snapshot: (md) => typings(md, task(md)),
  };
};

/** A new-design video page: the id is the model in its `POST /<family>/<model>` path. */
const videoPage = (key, page, family) => {
  const task = (md) => {
    const t = createTask(md);
    const m = t.endpoint.match(new RegExp(`^POST /${family}/(kling-[\\w.-]+)$`));
    if (!m) throw new Error(`page documents ${t.endpoint}, expected POST /${family}/<model> — the page moved to another API`);
    return { ...t, model: m[1] };
  };
  return {
    key,
    url: `${DOCS}/api/${page}.md`,
    expect: 'text',
    extract: (md) => [task(md).model],
    snapshot: (md) => typings(md, task(md)),
  };
};

/** A capability map: its per-model tables, verbatim minus prose descriptions. */
const capabilityMap = (key, page, tables) => ({
  key,
  index: true,
  url: `${DOCS}/guides/capability-map/${page}.md`,
  expect: 'text',
  extract: (md) => table(section(md, 'Models')).slice(2).map((l) => cells(l)[0].toLowerCase().replace(/\s+/g, '-')),
  snapshot: (md) => tables.map((t) => `# ${t}\n${table(section(md, t)).join('\n')}`).join('\n\n') + '\n',
});

export default {
  provider: 'kling',
  // Image ids: the legacy `model_name` the image adapter sends
  // (litegen-core/src/providers/image/kling.rs resolve_model strips `kling/`).
  // Video ids: the new API's path model for the same version. The video adapter
  // (video/kling.rs resolve_model strips `kling/` and `video-`) sends the legacy
  // `model_name` noted on each line, which Kling no longer documents. Keeping
  // video in the path namespace also separates the ids the legacy API shared
  // across endpoints (kling-v3 was both Image 3.0 and Video 3.0, kling-v2-1
  // both Image 2.1 and Video 2.1), so a retired video model can show as missing.
  models: {
    'kling/kling-v2-1': 'kling-v2-1',
    'kling/kling-v3': 'kling-v3',
    'kling/video-kling-v2-5-turbo': 'kling-2.5-turbo', // sends kling-v2-5-turbo
    'kling/video-kling-v2-6': 'kling-2.6', // sends kling-v2-6
    'kling/video-kling-v3': 'kling-3.0', // sends kling-v3
  },
  sources: [
    legacyPage('images', 'image/2-1/image-generation', 'POST /v1/images/generations'),
    videoPage('t2v-2-5-turbo', 'video/2-5-turbo/text-to-video', 'text-to-video'),
    videoPage('i2v-2-5-turbo', 'video/2-5-turbo/image-to-video', 'image-to-video'),
    videoPage('t2v-2-6', 'video/2-6/text-to-video', 'text-to-video'),
    videoPage('i2v-2-6', 'video/2-6/image-to-video', 'image-to-video'),
    videoPage('t2v-3-0', 'video/3-0-omni/text-to-video', 'text-to-video'),
    videoPage('i2v-3-0', 'video/3-0-omni/image-to-video', 'image-to-video'),
    videoPage('t2v-3-0-turbo', 'video/3-0-turbo/text-to-video', 'text-to-video'),
    videoPage('i2v-3-0-turbo', 'video/3-0-turbo/image-to-video', 'image-to-video'),
    capabilityMap('capability-video', 'video', ['Models', 'Text to Video', 'Image to Video']),
    capabilityMap('capability-image', 'image', ['Models', 'Text to Image', 'Image to Image']),
    {
      key: 'pages',
      index: true,
      url: 'https://app.klingai.com/global/dev/document-api/llms.txt',
      expect: 'text',
      extract: /\/document-api\/api\/((?:video|image)\/[\w./-]+?)\.md/g,
    },
  ],
  // kling-v1, kling-v2-master, kling-v2-1-master and kling-v2-new (skipped
  // 2026-09-11: retiring 2026-09-15) retired and left every source with the
  // legacy video docs, so their acknowledgements are gone too.
  acknowledged: [],
};
