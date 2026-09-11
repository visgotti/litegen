// ByteDance Seedream / Seedance on BytePlus ModelArk (international) — the
// docs center server-renders each page with its markdown embedded in
// `window._ROUTER_DATA` (curDoc.MDContent); that markdown is what we read.
//   model-list    the Model list page, which the API reference names as the
//                 place to look up a `model` value: its Video/Image generation
//                 tables are the ids we compare
//   video-api     index: POST /contents/generations/tasks request body
//   image-api     index: POST /images/generations request body
//                 (both snapshot parameter signatures, plus the per-model
//                 value notes for our models)
//   deprecations  index: deprecation batches that include our models
// Mapping mirrors litegen-core/src/providers/image/bytedance.rs `ark_model_id`
// (shared by video/bytedance.rs) for the default BytePlus host: strip
// `bytedance/` and any `doubao-` prefix; sent as body `model`.
//
// BytePlus documents Seedance without the `doubao-` prefix our video catalog
// ids carry (seedance-1-0-pro-250528); `doubao-…` is the Volcengine (China) id
// form, which the adapter now sends only to a *.volces.com api_base.

const DOCS = 'https://docs.byteplus.com/en/docs/ModelArk';

const unescapeMd = (s) => s.replace(/\\([\\`*_{}[\]()#+\-.!|~<>])/g, '$1');
const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());

/** The page's markdown, from the SSR payload; throws if the docs center changes shape. */
function pageMarkdown(html) {
  const m = html.match(/window\._ROUTER_DATA\s*=\s*/);
  if (!m) throw new Error('window._ROUTER_DATA not found — the docs center changed its server-rendered payload');
  const start = m.index + m[0].length;
  const data = JSON.parse(html.slice(start, html.indexOf('</script>', start)).trim().replace(/;$/, ''));
  const md = data?.loaderData?.['(lang)/docs/(libcode)/(doccode$)/page']?.curDoc?.MDContent;
  if (typeof md !== 'string' || md.length === 0) throw new Error('curDoc.MDContent missing — the docs center changed its payload');
  return md;
}

/** Lines under a heading (exact line), up to the next heading at the same or higher level. */
function section(md, heading) {
  const lines = md.split('\n');
  const start = lines.findIndex((l) => l.trim() === heading);
  if (start < 0) throw new Error(`section "${heading}" not found — the page was restructured`);
  const level = heading.match(/^#+/)[0].length;
  const end = lines.findIndex((l, i) => i > start && /^#+ /.test(l) && l.match(/^#+/)[0].length <= level);
  return lines.slice(start + 1, end < 0 ? undefined : end).map(unescapeMd);
}

/** Brand-free slug, so "Dreamina Seedance 1.0 pro" and doubao-seedance-1-0-pro-250528 compare. */
const slug = (s) =>
  s.toLowerCase().replace(/\(.*?\)/g, '').trim().replace(/^(?:doubao|bytedance|dreamina|dola)[\s-]+/, '').replace(/[\s.]+/g, '-');
const isOurs = (display, ours) => {
  const s = slug(display);
  return s !== '' && ours.some((id) => `${slug(id)}-`.startsWith(`${s}-`));
};

/**
 * Request-body digest of an API reference page: every parameter signature
 * (`name` `type` `flags`), its model-independent value bullets, and the
 * per-model value notes (bullets and tables) for our models only.
 */
function requestDigest(md, ours) {
  const out = [];
  let scope = 'param'; // 'param' → under a parameter; 'ours' / 'other' → inside a per-model block
  for (const l of section(md, '### Request body')) {
    const sig = l.match(/^\*\*([\w.[\]]+)\*\*((?:\s+`[^`]+`)+)/);
    const modelBlock = l.match(/^\*\*([^*]+?)\*\*\s*$/);
    const modelBullet = l.match(/^\s*\*\s+\*\*([^*]+?):?\*\*:?\s*(.*)$/);
    if (sig) {
      out.push(`${sig[1]}${sig[2].replace(/\s+/g, ' ')}`);
      scope = 'param';
    } else if (modelBlock) {
      scope = isOurs(modelBlock[1], ours) ? 'ours' : 'other';
      if (scope === 'ours') out.push(`  [${modelBlock[1].trim()}]`);
    } else if (modelBullet && /\d/.test(modelBullet[1])) {
      if (isOurs(modelBullet[1], ours)) out.push(`  ${modelBullet[1].trim()}: ${modelBullet[2].trim()}`);
    } else if (scope !== 'other' && /^\s*\*\s+(?:Supported|Default|Available|Optional|Valid)\b/.test(l) && !l.includes('<')) {
      out.push(`  ${l.trim().replace(/^\*\s+/, '')}`);
    } else if (scope === 'ours' && (l.startsWith('|') || (/^\s*\*\s/.test(l) && !l.includes('<')))) {
      out.push(`  ${l.trim()}`);
    }
  }
  if (!out.some((o) => /^model\b/.test(o))) throw new Error('no `model` parameter in the request body — the page was restructured');
  return out.join('\n') + '\n';
}

const paramNames = (md) => section(md, '### Request body').flatMap((l) => l.match(/^\*\*([\w.[\]]+)\*\*\s+`/)?.[1] ?? []);

const apiPage = (key, code) => ({
  key,
  index: true,
  url: `${DOCS}/${code}`,
  expect: 'html',
  extract: (_html, body) => paramNames(pageMarkdown(body)),
  snapshot: (_html, body, { ours }) => requestDigest(pageMarkdown(body), ours),
});

export default {
  provider: 'bytedance',
  models: {
    'bytedance/seedream-4-0-250828': 'seedream-4-0-250828',
    'bytedance/seedream-3-0-t2i-250415': 'seedream-3-0-t2i-250415',
    'bytedance/doubao-seedance-1-0-pro-250528': 'seedance-1-0-pro-250528',
    'bytedance/doubao-seedance-1-0-lite-i2v-250428': 'seedance-1-0-lite-i2v-250428',
  },
  sources: [
    {
      key: 'model-list',
      url: `${DOCS}/1330310`,
      expect: 'html',
      // First cell: [model-id](console link), sometimes "(also supports: other-id)".
      extract: (_html, body) => {
        const md = pageMarkdown(body);
        return ['# Video generation', '# Image generation']
          .flatMap((h) => section(md, h))
          .filter((l) => l.startsWith('|'))
          .flatMap((l) => {
            const first = cells(l)[0] ?? '';
            const also = first.match(/also supports:\s*([^)]*)\)/i)?.[1].split(/,|\band\b/) ?? [];
            return [...[...first.matchAll(/\[([^\]]+)\]\(/g)].map((m) => m[1]), ...also].map((s) => s.trim());
          });
      },
      // Our models' rows without the rate-limit column (quota changes are not typings).
      snapshot: (_html, body, { ours }) => {
        const md = pageMarkdown(body);
        const rows = ['# Video generation', '# Image generation'].flatMap((h) =>
          section(md, h)
            .filter((l) => l.startsWith('|'))
            .map(cells)
            .filter((c) => ours.some((id) => (c[0] ?? '').includes(`[${id}]`)))
            .map((c) => `| ${c.slice(0, h === '# Video generation' ? 3 : 2).join(' | ')} |`),
        );
        return rows.join('\n') + '\n';
      },
    },
    apiPage('video-api', '1520757'),
    apiPage('image-api', '1541523'),
    {
      key: 'deprecations',
      index: true,
      url: `${DOCS}/1350667`,
      expect: 'html',
      extract: (_html, body) =>
        pageMarkdown(body)
          .split('\n')
          .map(unescapeMd)
          .filter((l) => l.startsWith('|'))
          .map((l) => (cells(l)[0] ?? '').replace(/[`*]|（.*?）|\(.*?\)/g, '').trim())
          .filter((id) => /^[a-z][a-z0-9.]*(?:-[a-z0-9.]+)+$/.test(id)),
      // Each batch that lists one of our models: its heading, timetable dates, and those rows.
      snapshot: (_html, body, { ours }) => {
        const want = ours.map(slug);
        const out = [];
        let batch = null;
        let dates = null;
        let printed = null;
        for (const l of pageMarkdown(body).split('\n').map(unescapeMd)) {
          if (/^#+ /.test(l)) {
            if (/batch/i.test(l)) batch = l.replace(/[#*]/g, '').trim();
            continue;
          }
          if (!l.startsWith('|')) continue;
          const c = cells(l);
          if (/^[A-Z][a-z]+ \d{1,2}, \d{4}$/.test(c[0] ?? '')) dates = c.slice(0, 3).join(' / ');
          const id = slug((c[0] ?? '').replace(/[`*]|（.*?）/g, ''));
          if (!want.some((w) => id === w || id.endsWith(`-${w}`))) continue;
          if (printed !== batch) {
            out.push('', `# ${batch}`, `notified / deprecated / deactivated: ${dates}`);
            printed = batch;
          }
          out.push(`| ${c.join(' | ')} |`);
        }
        return out.length ? out.join('\n').trim() + '\n' : '(none of our models is in a deprecation batch)\n';
      },
    },
  ],
  acknowledged: [],
};
