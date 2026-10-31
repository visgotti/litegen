// ByteDance Seedream / Seedance on BytePlus ModelArk (international).
//
// Every source reads the docs center's own content API, which returns a page's
// markdown as JSON (Result.MDContent): the same markdown the HTML page embeds
// in `window._ROUTER_DATA`. We don't read the HTML pages:
//   - The docs center (Modern.js) sometimes falls back to client-side
//     rendering. It then serves the bare CSR template
//     (`<div id="root"><!--<?- html ?>--></div>`, no _ROUTER_DATA) with a 200,
//     on about 1 fetch in 10 (2026-10-12: 5 of 48 fetches across two pages).
//   - Since 2026-09-25, every numeric /docs/ModelArk/<code> URL 302s to a slug
//     URL (1330310 → /docs/modelark/model-list).
// The API served the same markdown on all 60 fetches across six pages
// (2026-10-12). `x-language: en` pins the English page; `zh` has no record
// for these slugs. Human-readable page: https://docs.byteplus.com/en/docs/modelark/<code>
//   model-list    the Model list page, which the API reference names as the
//                 place to look up a `model` value: its Video/Image generation
//                 tables are the ids we compare
//   video-api     index: POST /contents/generations/tasks request body
//   task-api      index: GET /contents/generations/tasks/{id} response (the
//                 video adapter's poll: status, content.video_url, error)
//   image-api     index: POST /images/generations request body
//                 (the three API pages snapshot parameter signatures, values,
//                 and the per-model notes for our models)
//   pricing       index: our models' rows of the video and image price tables
//   deprecations  index: deprecation batches and notes that name our models
// Mapping mirrors litegen-core/src/providers/image/bytedance.rs `ark_model_id`
// (shared by video/bytedance.rs) for the default BytePlus host: strip
// `bytedance/` and any `doubao-` prefix; sent as body `model`.
//
// BytePlus documents Seedance without the `doubao-` prefix our video catalog
// ids carry (seedance-1-0-pro-250528); `doubao-…` is the Volcengine (China) id
// form, which the adapter now sends only to a *.volces.com api_base.

const DOC_API = 'https://www.byteplus.com/api/doc/getDocDetail?LibraryCode=modelark&DocumentCode=';
const HEADERS = { 'x-language': 'en' };

const unescapeMd = (s) => s.replace(/\\([\\`*_{}[\]()#+\-.!|~<>])/g, '$1');
const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());
/** A markdown line as plain text: no "new" badges, HTML tags, or bold markers. */
const plain = (s) =>
  s
    .replace(/<mark>.*?<\/mark>/g, '')
    .replace(/<br\s*\/?>/g, ' ')
    .replace(/<[^>]+>/g, '')
    .replace(/\*\*/g, '')
    .replace(/\s+/g, ' ')
    .trim();

/** The page's markdown from the docs API; throws, with the reason, on anything else. */
function pageMarkdown(json) {
  const err = json?.ResponseMetadata?.Error;
  if (err) throw new Error(`docs API error ${err.Code}: ${err.Message}`);
  const moved = json?.Result?.RedirectInfo;
  if (moved?.RedirectDocumentCode) {
    throw new Error(`the page moved to ${moved.RedirectLibraryCode}/${moved.RedirectDocumentCode}: update its DocumentCode`);
  }
  const md = json?.Result?.MDContent;
  if (typeof md !== 'string' || md.length === 0) throw new Error('Result.MDContent missing: the docs API changed its payload');
  return unbadge(md);
}

/**
 * The docs CMS glues a "new" badge to the bold run it marks
 * (`**layer_decompositionnew** \`boolean\``, "Dreamina Seedance 2.5new"): a
 * badge is not part of a name. A name must keep four characters before it.
 */
const unbadge = (md) =>
  md
    .replace(/<mark>.*?<\/mark>/g, '')
    .replace(/\*\*([A-Za-z_][\w.[\]]{3,}?)new\*\*(?=\s+`)/g, '**$1**') // a parameter signature
    .replace(/\*\*([^*\n]*\d[^*\n]*?)new\*\*/g, '**$1**'); // a model name with a version

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
const isModelName = (s) => /seed(?:ream|ance)/i.test(s);
/**
 * Whether a display name ("Seedance 1.0 pro", "Dreamina Seedance 2.0 series")
 * names one of our models: an exact match on the dateless id, or for a
 * "… series" name, any of ours in that series. "Seedance 1.0 pro" is not
 * "Seedance 1.0 pro fast", and "Dreamina Seedance 2.0" is not 2.0 mini.
 */
const isOurs = (display, ours) => {
  const s = slug(plain(display).replace(/[`:.]+$/g, '').replace(/`/g, ''));
  if (!s) return false;
  const series = s.match(/^(.+)-series$/)?.[1];
  return ours.some((id) => {
    const bare = slug(id).replace(/-\d{6}$/, '');
    return series ? `${bare}-`.startsWith(`${series}-`) : bare === s;
  });
};
/**
 * "A, B series (1–9 images), and C." or "Seedream 4.5 / 4.0" → the names among
 * them that are ours. A bare version ("4.0") takes the family of the name
 * before it.
 */
const oursIn = (list, ours) => {
  let family = '';
  return plain(list)
    .replace(/\.$/, '')
    .split(/\s*\/\s*|,\s*(?:and\s+)?|\s+and\s+/)
    .map((n) => n.replace(/`/g, '').trim())
    .map((n) => {
      if (/^\d/.test(n)) return `${family}${n}`;
      family = n.match(/^\D+/)?.[0] ?? '';
      return n;
    })
    .filter((n) => /^all models$/i.test(n) || isOurs(n, ours));
};

/**
 * Parameter digest of an API reference section: every parameter signature
 * (`name` `type` `flags`), the values each takes, which of our models each is
 * supported for, and the per-model value notes, bullets and tables for our
 * models only. Prose is dropped: it churns without changing a typing.
 */
function paramDigest(md, heading, ours) {
  const out = [];
  let scope = 'param'; // 'param': under a parameter · 'ours' / 'other': inside a per-model block
  let label = null; // the last scenario line, e.g. "Image-to-video (first and last frames)"
  let list = null; // collecting a "Supported models:" bullet list
  let values = []; // consecutive "* `value`: …" bullets
  let table = null; // { keep: column indices } | { rows: true } (rows named by model) | { skip: true }

  const supported = (names) => {
    const ok = oursIn(names.join(', '), ours);
    out.push(`  ${label ? `[${label}] ` : ''}supported models: ${ok.length ? ok.join(', ') : '(none of ours)'}`);
    label = null;
  };
  const flush = () => {
    if (list?.length) supported(list); // an empty list is followed by per-model bullets instead
    list = null;
    if (values.length) out.push(`  values: ${values.map((v) => `\`${v}\``).join(', ')}`);
    values = [];
  };

  for (const line of section(md, heading)) {
    const l = line.replace(/<mark>.*?<\/mark>/g, '');
    const t = l.trim();
    if (!t || t === '&nbsp;') continue;

    // A "Supported models:" heading, then one `Model` bullet per line.
    if (list && /^\*\s+`[^`]+`$/.test(t)) {
      list.push(t.match(/`([^`]+)`/)[1]);
      continue;
    }
    if (scope === 'param' && /^\s*\*\s+`[^`]+`/.test(l)) {
      values.push(t.match(/`([^`]+)`/)[1]);
      continue;
    }
    flush();
    if (!t.startsWith('|')) table = null;

    const sig = l.match(/^\*\*([\w.[\] ]+?)\*\*((?:\s+`[^`]+`)+)/);
    if (sig) {
      out.push(`${sig[1]}${sig[2].replace(/\s+/g, ' ')}`);
      scope = 'param';
      label = null;
      continue;
    }
    const scenario = l.match(/^\*\*([^*]+?)\s*\*\*\s+\*\*`([^`]+)`\*\*\s*$/);
    if (scenario) {
      out.push(`  [${scenario[1].trim()}] ${scenario[2]}`);
      continue;
    }
    if (/^\*\*Supported models?:?\*\*:?$/i.test(t)) {
      list = [];
      continue;
    }
    const inline = t.match(/^(?:\*\s+)?\*\*Supported models?:?\*\*:?\s+(.+)$/i) ?? t.match(/^>\s*Supported by (.+)$/i);
    if (inline) {
      if (scope !== 'other') supported([inline[1]]);
      continue;
    }
    const block = t.match(/^\*\*([^*]+?)\*\*$/);
    if (block && isModelName(block[1])) {
      scope = oursIn(block[1], ours).length ? 'ours' : 'other';
      if (scope === 'ours') out.push(`  [${block[1].trim()}]`);
      continue;
    }
    const perModel = l.match(/^\s*\*\s+\*\*([^*]+?):?\*\*:?\s*(.*)$/);
    if (perModel && isModelName(perModel[1]) && /\d/.test(perModel[1])) {
      if (oursIn(perModel[1], ours).length) out.push(`  ${perModel[1].trim()}: ${plain(perModel[2])}`);
      continue;
    }
    if (scope === 'other') continue;

    if (t.startsWith('|')) {
      const c = cells(t);
      if (scope === 'ours') out.push(`  ${t}`);
      else if (c.every((x) => /^-+$/.test(x))) continue;
      else if (!table) {
        // The header decides: a column per model (keep ours), or a row per model.
        if (/^model\b/i.test(plain(c[0] ?? ''))) table = { rows: true };
        else if (c.some(isModelName)) table = { keep: c.flatMap((x, i) => (!isModelName(x) || oursIn(x, ours).length ? [i] : [])) };
        else table = { skip: true };
        if (!table.skip) out.push(`  | ${(table.keep ? table.keep.map((i) => c[i]) : c).map(plain).join(' | ')} |`);
      } else if (table.rows) {
        if (oursIn(c[0] ?? '', ours).length) out.push(`  | ${c.map(plain).join(' | ')} |`);
      } else if (table.keep) out.push(`  | ${table.keep.map((i) => plain(c[i] ?? '')).join(' | ')} |`);
      continue;
    }
    if (l.includes('<')) continue; // HTML tips are prose
    const bullet = t.startsWith('*');
    if (/^(?:\*\s+)?(?:Supported|Default|Available|Optional|Valid)\b/.test(t)) {
      out.push(`  ${plain(t.replace(/^\*\s+/, ''))}`);
    } else if (scope === 'ours') {
      if (bullet) out.push(`  ${t}`);
    } else if (/^(?:\*\s+)?(?:\*\*)?Value range(?:\*\*)?\s*:/.test(t)) {
      out.push(`  ${plain(t.replace(/^\*\s+/, ''))}`);
    } else if (/\bup to \d+ reference\b/.test(t) && !bullet) {
      out.push(`  ${plain(t)}`); // "… support up to 14 reference images."
    } else if (/\b(?:set (?:the value |it )?to|Fixed to) `[^`]+`/.test(t) && !bullet) {
      out.push(`  value: \`${t.match(/\b(?:set (?:the value |it )?to|Fixed to) `([^`]+)`/)[1]}\``);
    } else if (/^\*\s+[^`]+:\s*`[^`]+`\.?$/.test(t)) {
      out.push(`  - ${plain(t.replace(/^\*\s+(?:>\s*)?/, ''))}`); // "Role of the first frame: `first_frame`"
    } else if (/^[A-Z][\w\- ()/,]{2,60}$/.test(plain(t)) && !bullet && !t.startsWith('>')) {
      label = plain(t);
    }
  }
  flush();
  return out;
}

function requestDigest(md, ours) {
  const out = paramDigest(md, '### Request body', ours);
  if (!out.some((o) => /^model\b/.test(o))) throw new Error('no `model` parameter in the request body — the page was restructured');
  return out.join('\n') + '\n';
}

/**
 * Model ids in a model-list first cell: [model-id](console link), sometimes a
 * second one, "(also supports: other-id)" or "<br><br>Also supports `other-id`".
 */
const MODEL_ID = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/;
const rowIds = (first) => {
  const also = [...first.matchAll(/also supports:?\s*\(?([^)|]*)/gi)].flatMap((m) => m[1].split(/,|\band\b|<br\s*\/?>/));
  // Status badges are links too ("[Retired](console link)"): keep id-shaped text only.
  return [...[...first.matchAll(/\[([^\]]+)\]\(/g)].map((m) => m[1]), ...also]
    .map((s) => s.replace(/`/g, '').trim())
    .filter((s) => MODEL_ID.test(s));
};

/**
 * The tables among `lines`, each labelled by the heading above it and any
 * short caption line between them ("Retiring models", "**Input without
 * video**"). A table is its run of `|` lines.
 */
function tablesIn(lines) {
  const out = [];
  let heading = '';
  let caption = '';
  let table = null;
  for (const l of lines) {
    if (l.startsWith('|')) {
      if (!table) out.push((table = { label: [heading, caption].filter(Boolean).join(' · '), rows: [] }));
      table.rows.push(l);
      continue;
    }
    table = null;
    const t = plain(l);
    if (/^#+ /.test(l)) {
      heading = l.replace(/^#+\s*/, '').trim();
      caption = '';
    } else if (t && t.length <= 40 && !/[.:]$/.test(t) && !l.startsWith('>')) {
      caption = t;
    }
  }
  return out;
}

/** A table row as plain text, links reduced to their text. */
const plainRow = (row) => `| ${cells(row).map((c) => plain(c.replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')).replace(/`/g, '')).join(' | ')} |`;

/** The lines of a `#`-heading inside `lines` (the first one), up to the next of the same or higher level. */
function subsection(lines, heading) {
  const start = lines.findIndex((l) => l.trim() === heading);
  if (start < 0) throw new Error(`section "${heading}" not found — the page was restructured`);
  const level = heading.match(/^#+/)[0].length;
  const end = lines.findIndex((l, i) => i > start && /^#+ /.test(l) && l.match(/^#+/)[0].length <= level);
  return lines.slice(start + 1, end < 0 ? undefined : end);
}

const paramNames = (md, heading) => section(md, heading).flatMap((l) => l.replace(/<mark>.*?<\/mark>/g, '').match(/^\*\*([\w.[\]]+)\*\*\s+`/)?.[1] ?? []);

const docPage = (key, code) => ({ key, url: `${DOC_API}${code}`, headers: HEADERS, expect: 'json' });
const apiPage = (key, code) => ({
  ...docPage(key, code),
  index: true,
  extract: (json) => paramNames(pageMarkdown(json), '### Request body'),
  snapshot: (json, _body, { ours }) => requestDigest(pageMarkdown(json), ours),
});

// The model list's media and the price page's: a `##` section each.
const MODEL_FAMILIES = ['## Video generation', '## Image generation'];
const PRICE_FAMILIES = ['## Video generation models', '## Image generation models'];
const cellId = (c) => plain(c ?? '').replace(/`/g, '').split(' ')[0];

/** Each medium's price tables, from its `### Pricing` subsection. */
const priceTables = (md) =>
  PRICE_FAMILIES.flatMap((h) =>
    tablesIn(subsection(section(md, h), '### Pricing')).map((t) => ({ ...t, label: t.label || 'Pricing', family: h.replace(/^#+\s*/, '') })),
  );

export default {
  provider: 'bytedance',
  models: {
    'bytedance/seedream-5-0-lite-260128': 'seedream-5-0-lite-260128',
    'bytedance/seedream-4-5-251128': 'seedream-4-5-251128',
    'bytedance/seedream-4-0-250828': 'seedream-4-0-250828',
    'bytedance/doubao-seedance-1-0-pro-250528': 'seedance-1-0-pro-250528',
    'bytedance/seedance-1-0-pro-fast-251015': 'seedance-1-0-pro-fast-251015',
    'bytedance/dreamina-seedance-2-0-mini-260615': 'dreamina-seedance-2-0-mini-260615',
  },
  sources: [
    {
      ...docPage('model-list', 'model-list'),
      extract: (json) => {
        const md = pageMarkdown(json);
        return MODEL_FAMILIES.flatMap((h) => section(md, h))
          .filter((l) => l.startsWith('|'))
          .flatMap((l) => rowIds(cells(l)[0] ?? ''));
      },
      // Each table but the rate limits (quota changes are not typings): its
      // label, header, and our models' rows. A row counts as ours when any of
      // its ids is (seedream-5-0-lite-260128 is an "also supports" id on the
      // seedream-5-0-260128 row), so a "Retired" badge, or a move into the
      // "Retiring models" table, shows as a change.
      snapshot: (json, _body, { ours }) => {
        const md = pageMarkdown(json);
        const out = [];
        for (const h of MODEL_FAMILIES) {
          for (const t of tablesIn(section(md, h)).filter((t) => !/^Limits\b/.test(t.label))) {
            const mine = t.rows.filter((r) => rowIds(cells(r)[0] ?? '').some((id) => ours.includes(id)));
            if (mine.length) out.push(`# ${h.replace(/^#+\s*/, '')} · ${t.label}`, plainRow(t.rows[0]), ...mine.map(plainRow), '');
          }
        }
        if (out.length === 0) throw new Error('none of our models is in the model list tables — the page was restructured');
        return out.join('\n');
      },
    },
    apiPage('video-api', 'create-video-generation-task-api'),
    {
      ...docPage('task-api', 'get-video-generation-task-api'),
      index: true,
      extract: (json) => paramNames(pageMarkdown(json), '## Response parameters'),
      snapshot: (json, _body, { ours }) => {
        const out = paramDigest(pageMarkdown(json), '## Response parameters', ours);
        if (!out.some((o) => /^status\b/.test(o))) throw new Error('no `status` in the task response — the page was restructured');
        return out.join('\n') + '\n';
      },
    },
    apiPage('image-api', 'image-generation-api'),
    {
      ...docPage('pricing', 'model-pricing'),
      index: true,
      extract: (json) =>
        priceTables(pageMarkdown(json))
          .flatMap((t) => t.rows.map((r) => cellId(cells(r)[0])))
          .filter((id) => MODEL_ID.test(id)),
      // Each rate table that prices one of ours: its label, header rows (a
      // second one has an empty first cell), and our rows; then the billing
      // examples our flat prices are read from.
      snapshot: (json, _body, { ours }) => {
        const md = pageMarkdown(json);
        const out = [];
        for (const t of priceTables(md)) {
          const mine = t.rows.filter((r) => ours.includes(cellId(cells(r)[0])));
          if (!mine.length) continue;
          const head = t.rows.filter((r, i) => i === 0 || (!cellId(cells(r)[0]) && !/^\|\s*-+/.test(r) && !/^\|-/.test(r)));
          out.push(`# ${t.family} · ${t.label}`, ...head.map(plainRow), ...mine.map(plainRow), '');
        }
        if (out.length === 0) throw new Error('none of our models is in the price tables — the page was restructured');
        const examples = section(md, '## Video generation models');
        const at = examples.findIndex((l) => l.trim() === '### Billing examples');
        if (at >= 0) {
          for (const t of tablesIn(examples.slice(at))) {
            if (isOurs(t.label.split(' · ')[0], ours)) out.push(`# billing example · ${t.label}`, ...t.rows.filter((r) => !/^\|-/.test(r)).map(plainRow), '');
          }
        }
        return out.join('\n');
      },
    },
    {
      ...docPage('deprecations', 'model-deprecation-notice'),
      index: true,
      extract: (json) =>
        pageMarkdown(json)
          .split('\n')
          .map(unescapeMd)
          .filter((l) => l.startsWith('|'))
          .map((l) => (cells(l)[0] ?? '').replace(/[`*]|（.*?）|\(.*?\)/g, '').trim())
          .filter((id) => /^[a-z][a-z0-9.]*(?:-[a-z0-9.]+)+$/.test(id)),
      // Each batch that names one of our models: its heading, timetable dates, the
      // table rows whose model is ours, and any note line or bullet naming ours
      // (such as a region-only deactivation), with the line that introduces it.
      snapshot: (json, _body, { ours }) => {
        // An id of ours as a whole token, with or without a brand prefix
        // (bytedance-seedream-…, dola-seedream-…, doubao-seedance-…).
        const res = ours.map(
          (id) => new RegExp(`(?:^|[^a-z0-9-])(?:(?:doubao|bytedance|dreamina|dola)-)?${slug(id).replace(/[.-]/g, '\\$&')}(?![a-z0-9-])`),
        );
        const names = (s) => res.some((re) => re.test(plain(s).toLowerCase()));
        const out = [];
        let batch = null;
        let dates = null;
        let printed = null;
        let intro = null;
        let introPrinted = null;
        const header = () => {
          if (printed === batch) return;
          out.push('', `# ${batch ?? '(no batch heading)'}`, `notified / deprecated / deactivated: ${dates}`);
          printed = batch;
        };
        // Notes are blockquotes now (`> …`, `* > …`); the quote is not content.
        for (const l of pageMarkdown(json)
          .split('\n')
          .map(unescapeMd)
          .map((x) => x.replace(/^(\s*\*\s+)?>\s?/, '$1').replace(/`/g, ''))) {
          const t = plain(l);
          if (!t) continue;
          if (/^#+ /.test(l)) {
            if (/batch/i.test(l)) batch = l.replace(/[#*]/g, '').trim();
            intro = null;
            continue;
          }
          if (l.startsWith('|')) {
            const c = cells(l);
            if (/^[A-Z][a-z]+ \d{1,2}, \d{4}$/.test(c[0] ?? '')) dates = c.slice(0, 3).join(' / ');
            if (!names(c[0] ?? '')) continue;
            header();
            out.push(`| ${c.join(' | ')} |`);
          } else if (/^\s*\*\s/.test(l)) {
            if (!names(t)) continue;
            header();
            if (intro && introPrinted !== intro) out.push(`  ${intro}`);
            introPrinted = intro;
            out.push(`  - ${t.replace(/^\*\s+/, '')}`);
          } else if (names(t)) {
            header();
            out.push(`  ${t}`);
          } else {
            intro = t;
          }
        }
        return out.length ? out.join('\n').trim() + '\n' : '(none of our models is in a deprecation batch)\n';
      },
    },
  ],
  acknowledged: [
    {
      id: 'seedance-1-5-pro-251215',
      reason: 'skipped 2026-09-11: in the fourth deprecation batch, deactivated 2026-11-11 (replacement dreamina-seedance-2-0-mini-260615, carried)',
    },
    {
      id: 'dreamina-seedance-2-0-260128',
      reason: 'skipped 2026-09-11: optional; the Seedance 2.0 series needs a USD 30 BytePlus balance to activate (2.0 mini is carried)',
    },
    {
      id: 'dreamina-seedance-2-0-fast-260128',
      reason: 'skipped 2026-09-11: optional; the Seedance 2.0 series needs a USD 30 BytePlus balance to activate (2.0 mini is carried)',
    },
    {
      id: 'dreamina-seedance-2-5-260628',
      reason: 'skipped 2026-09-11: image-to-video only accepts ratio `adaptive`; skip for now',
    },
    {
      id: 'dola-seedream-5-0-pro-260628',
      reason: 'skipped 2026-09-11: non-lite Seedream 5.0 (pro) is optional; seedream-5-0-lite-260128 is carried',
    },
    {
      id: 'seedream-5-0-260128',
      reason: 'skipped 2026-09-11: non-lite Seedream 5.0 is optional; seedream-5-0-lite-260128 is carried',
    },
  ],
};
