// Tencent TokenHub (大模型服务平台 TokenHub, docs product 1823): Tencent's model
// hub and Hunyuan's new home ("腾讯混元大模型相关功能将逐步迁移至 TokenHub。迁移后，原平台
// 将不再新增模型能力，并停止支持新购模型服务。" — the hunyuan / vclm / aiart billing
// pages). One Bearer API key, host https://tokenhub.tencentmaas.com. The old
// Tencent Cloud API stays as `hunyuan` (providers/hunyuan.mjs) for the buyers
// who still use it.
//
// Tencent's docs center server-renders these pages (Slate HTML). Sources:
//   models      模型列表: the image, video and 3D generation tables — every
//               `model（调用参数）` id TokenHub serves. A new id shows up here
//               first; a retirement shows as a changed snapshot (the name
//               gains "（YYYY-MM-DD 下线）").
//   hy-image    Hy 生图调用指南: hy-image-v3.5-preview's endpoint and parameter
//               tables (the adapter's contract) and the reference limits.
//   hy-video    Hy 生视频调用指南: hy-video-v1.5's submit and query tables.
//   wand-video  WAND 生视频调用指南: wand-vega-video-1.0-lite's submit (content
//               parts and roles) and query tables.
//   prices      模型价格: every image, video and 3D price row, so a price change
//               shows as `changed`.
//
//   kling-video Kling 生视频调用指南: the resold Kling models' text-to-video,
//               image-to-video and task-query tables.
//   seedream-image  Seedream 生图调用指南: the resold Seedream models' table.
//   minimax-video   MiniMax 生视频调用指南: v2.3's flat and H3's content shapes.
//   pixverse-video  PixVerse 生视频调用指南: its four endpoints and the task query.
//   kling-image     Kling 生图调用指南: v3 and the omni models' fields, the task query.
//   vidu-image      Vidu 生图调用指南: vidu-image-q2's fields and the task query.
//   vidu-video      Vidu 生视频调用指南: the four endpoints, the task query and the
//                   duration table (各接口时长取值速查).
//
// TokenHub's resold models are carried as TokenHub routes even where litegen
// also calls the vendor direct (the user, 2026-11-04: "like how litellm can
// still use openrouter"); skips name a real reason (a shutdown, a feature
// litegen has no shape for, an unverifiable 3D delivery).
//
// Mapping mirrors litegen-core/src/providers/image/tokenhub.rs and
// video/tokenhub.rs (`route`: the endpoint and the `model` value sent).

const DOCS = 'https://cloud.tencent.com/document/product/1823';

/** Acknowledge a list of upstream ids with one reason. */
const skip = (reason, ids) => ids.map((id) => ({ id, reason: `skipped 2026-11-04: ${reason}` }));


const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ' };

/** Visible text of an HTML fragment, whitespace collapsed (zero-width and BOM characters dropped). */
const htmlText = (s) =>
  s
    .replace(/<br\s*\/?>|<\/(?:div|p|li)>/gi, ' ')
    .replace(/<[^>]+>/g, '')
    .replace(/&(?:#(\d+)|#x([0-9a-f]+)|(\w+));/gi, (m, dec, hex, name) =>
      dec ? String.fromCodePoint(+dec) : hex ? String.fromCodePoint(parseInt(hex, 16)) : (ENTITIES[name.toLowerCase()] ?? m),
    )
    .replace(/[​‌‍﻿]/g, '')
    .replace(/\s+/g, ' ')
    .trim();

/**
 * A docs page as its structural blocks, in document order: headings (h2-h4),
 * tables (rows of cell text) and endpoint lines (a `<code>` that is exactly
 * "GET|POST https://…"). Prose paragraphs are left out: they churn, and the
 * contract is in the tables.
 */
function docBlocks(html) {
  const body = html.replace(/<script\b[\s\S]*?<\/script>/gi, '');
  const blocks = [];
  for (const m of body.matchAll(/<h([2-4])\b[^>]*>([\s\S]*?)<\/h\1>|<table\b[\s\S]*?<\/table>|<code>([\s\S]*?)<\/code>/gi)) {
    if (m[1]) {
      blocks.push({ kind: 'heading', level: Number(m[1]), text: htmlText(m[2]) });
    } else if (m[3] === undefined) {
      const rows = [...m[0].matchAll(/<tr\b[\s\S]*?<\/tr>/gi)].map((r) =>
        [...r[0].matchAll(/<t[hd]\b[^>]*>([\s\S]*?)<\/t[hd]>/gi)].map((c) => htmlText(c[1]).replace(/\|/g, '\\|')),
      );
      blocks.push({ kind: 'table', rows });
    } else if (/^(?:GET|POST) https:\/\/\S+$/.test(htmlText(m[3]))) {
      blocks.push({ kind: 'endpoint', text: htmlText(m[3]) });
    }
  }
  if (!blocks.some((b) => b.kind === 'table')) throw new Error('no tables on the page — the docs center stopped server-rendering it, or it moved');
  return blocks;
}

/** The `model` column of the page's model-list table ("model 参数值" / "model 参数值（模型 ID）"). */
function tokenhubModelIds(html) {
  const table = docBlocks(html).find((b) => b.kind === 'table' && b.rows[0]?.some((c) => /^model 参数值/.test(c)));
  if (!table) throw new Error('no model list table (a "model 参数值" column) — the page was restructured');
  const col = table.rows[0].findIndex((c) => /^model 参数值/.test(c));
  const ids = table.rows.slice(1).map((r) => r[col] ?? '');
  const bad = ids.filter((id) => !/^[a-z][\w.-]*$/i.test(id));
  if (bad.length) throw new Error(`model list cells that are not ids: ${bad.join(', ')} — the table layout changed`);
  return ids;
}

/**
 * The page's endpoints and tables under the h2 sections `keep` matches, each
 * headed by its "h2 › h3 › h4" path, skipping h3 subsections `drop` matches
 * (error codes, request/response examples). Throws when a kept section is gone.
 */
function tokenhubExcerpt(html, { keep, drop }) {
  const out = [];
  const path = [];
  const seen = new Set();
  let printed = '';
  for (const b of docBlocks(html)) {
    if (b.kind === 'heading') {
      path.length = b.level - 2;
      path[b.level - 2] = b.text;
      continue;
    }
    const h2 = path[0] ?? '';
    const pattern = keep.find((k) => k.test(h2));
    if (!pattern || (path[1] && drop.test(path[1]))) continue;
    seen.add(pattern);
    const at = path.filter(Boolean).join(' › ');
    if (at !== printed) out.push(`\n## ${at}`);
    printed = at;
    out.push(b.kind === 'endpoint' ? b.text : b.rows.map((r) => `| ${r.join(' | ')} |`).join('\n'));
  }
  const gone = keep.filter((k) => !seen.has(k));
  if (gone.length) throw new Error(`no endpoint or table under a section matching ${gone.join(', ')} — the page was restructured`);
  return out.join('\n').trim() + '\n';
}

const NOT_CONTRACT = /错误码|示例/; // error-code and example subsections
/** Cells of a table row with its rowspan continuations' empty first cell kept empty. */
const firstCell = (row) => (row[0] ?? '').replace(/[​-‍﻿]/g, '').trim();

/**
 * The generation tables of the model list (h3 图像生成 / 视频生成 / 3D 生成 under
 * h2 视觉模型), as rows of [section, name, id, task types]. Throws when a
 * section or its `model（调用参数）` column is gone.
 */
function generationRows(html) {
  const rows = [];
  const seen = new Set();
  let h2 = '';
  let h3 = '';
  for (const b of docBlocks(html)) {
    if (b.kind === 'heading') {
      if (b.level === 2) [h2, h3] = [b.text, ''];
      if (b.level === 3) h3 = b.text;
      continue;
    }
    if (b.kind !== 'table' || h2 !== '视觉模型' || !/^(图像生成|视频生成|3D 生成)$/.test(h3)) continue;
    const head = b.rows[0];
    const col = head.findIndex((c) => /^model（调用参数）/.test(c));
    const task = head.findIndex((c) => c === '任务类型');
    if (col < 0) throw new Error(`the ${h3} table has no "model（调用参数）" column — the page was restructured`);
    seen.add(h3);
    for (const r of b.rows.slice(1)) rows.push([h3, r[0] ?? '', r[col] ?? '', task < 0 ? '' : (r[task] ?? '')]);
  }
  const gone = ['图像生成', '视频生成', '3D 生成'].filter((s) => !seen.has(s));
  if (gone.length) throw new Error(`no model table under 视觉模型 › ${gone.join(', ')} — the page was restructured`);
  const bad = rows.filter((r) => !/^[a-z][\w.-]*$/i.test(r[2]));
  if (bad.length) throw new Error(`model cells that are not ids: ${bad.map((r) => r[2]).join(', ')} — the table layout changed`);
  return rows;
}

/**
 * The 视觉模型 price tables' rows (rowspan continuations included), each
 * table headed by its section. Throws when none is found.
 */
function tencentPriceRows(html) {
  const out = [];
  let h3 = '';
  let h4 = '';
  for (const b of docBlocks(html)) {
    if (b.kind === 'heading') {
      if (b.level === 3) [h3, h4] = [b.text, ''];
      if (b.level === 4) h4 = b.text;
      continue;
    }
    if (b.kind !== 'table' || h3 !== '视觉模型' || !/^(图片生成|视频生成|3D 生成)$/.test(h4)) continue;
    let current = '';
    let printed = false;
    for (const r of b.rows.slice(1)) {
      const name = firstCell(r);
      if (name) current = name;
      if (!current) continue;
      if (!printed) out.push(`\n## ${h4} › ${b.rows[0].join(' | ')}`);
      printed = true;
      out.push(`| ${r.join(' | ')} |`);
    }
  }
  if (!out.length) throw new Error('no price rows under 视觉模型 — the price page was restructured');
  return out.join('\n').trim() + '\n';
}

export default {
  provider: 'tokenhub',
  models: {
    'tokenhub/hy-image-v3.5-preview': 'hy-image-v3.5-preview',
    'tokenhub/hy-video-v1.5': 'hy-video-v1.5',
    'tokenhub/wand-vega-video-1.0-lite': 'wand-vega-video-1.0-lite',
    'tokenhub/kling-video-v2.5-turbo': 'kling-video-v2.5-turbo',
    'tokenhub/kling-video-v2.6': 'kling-video-v2.6',
    'tokenhub/kling-video-v3': 'kling-video-v3',
    'tokenhub/kling-video-v3-turbo': 'kling-video-v3-turbo',
    'tokenhub/kling-video-v4': 'kling-video-v4',
    'tokenhub/seedream-image-v5.0-pro': 'seedream-image-v5.0-pro',
    'tokenhub/seedream-image-v5.0-flash': 'seedream-image-v5.0-flash',
    'tokenhub/minimax-video-v2.3': 'minimax-video-v2.3',
    'tokenhub/minimax-video-v2.3-fast': 'minimax-video-v2.3-fast',
    'tokenhub/minimax-video-h3': 'minimax-video-h3',
    'tokenhub/minimax-video-h3-max': 'minimax-video-h3-max',
    'tokenhub/pixverse-video-v6.0': 'pixverse-video-v6.0',
    'tokenhub/pixverse-video-c1': 'pixverse-video-c1',
    'tokenhub/pixverse-video-v5.6': 'pixverse-video-v5.6',
    'tokenhub/kling-image-v3': 'kling-image-v3',
    'tokenhub/kling-image-o1': 'kling-image-o1',
    'tokenhub/kling-image-v3-omni': 'kling-image-v3-omni',
    'tokenhub/vidu-image-q2': 'vidu-image-q2',
    'tokenhub/vidu-video-q4-preview': 'vidu-video-q4-preview',
    'tokenhub/vidu-video-q3-pro': 'vidu-video-q3-pro',
    'tokenhub/vidu-video-q3-turbo': 'vidu-video-q3-turbo',
    'tokenhub/vidu-video-q3': 'vidu-video-q3',
    'tokenhub/vidu-video-q2': 'vidu-video-q2',
    'tokenhub/vidu-video-q2-pro': 'vidu-video-q2-pro',
    'tokenhub/vidu-video-q2-turbo': 'vidu-video-q2-turbo',
    'tokenhub/yt-video-2.0': 'yt-video-2.0',
    'tokenhub/kling-video-v3-omni': 'kling-video-v3-omni',
    'tokenhub/kling-video-o1': 'kling-video-o1',
  },
  sources: [
    {
      key: 'models',
      url: `${DOCS}/130051`,
      expect: 'html',
      extract: (html) => generationRows(html).map((r) => r[2]),
      snapshot: (html) =>
        generationRows(html)
          .map(([section, name, id, task]) => `${section} | ${name} | ${id} | ${task}`)
          .join('\n') + '\n',
    },
    {
      key: 'hy-image',
      url: `${DOCS}/135745`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^Hy-Image-3\.5-preview /, /^附录$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'hy-video',
      url: `${DOCS}/137202`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^视频生成$/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'wand-video',
      url: `${DOCS}/138195`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^视频生成$/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'kling-video',
      url: `${DOCS}/135742`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^文生视频$/, /^图生视频$/, /^全能视频生成/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'seedream-image',
      url: `${DOCS}/136609`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^图片生成$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'minimax-video',
      url: `${DOCS}/135737`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^文生视频$/, /^图生视频/, /^多模态参考生视频/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'pixverse-video',
      url: `${DOCS}/135325`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^文生视频$/, /^图生视频$/, /^首尾帧生视频$/, /^参考生视频$/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'kling-image',
      url: `${DOCS}/138803`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^图片生成/, /^Omni 图片生成/, /^查询任务$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'vidu-image',
      url: `${DOCS}/135746`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^图片生成$/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'vidu-video',
      url: `${DOCS}/135756`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^文生视频$/, /^图生视频$/, /^多图参考生视频$/, /^首尾帧生视频$/, /^查询任务结果$/, /^附录$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'yt-video',
      url: `${DOCS}/135716`,
      expect: 'html',
      extract: () => ['yt-video-2.0', 'yt-video-humanactor'],
      snapshot: (html) => {
        const blocks = docBlocks(html).filter((b) => b.kind === 'table' || b.kind === 'endpoint');
        return blocks.map((b) => (b.kind === 'endpoint' ? b.text : b.rows.map((r) => `| ${r.join(' | ')} |`).join('\n'))).join('\n\n') + '\n';
      },
    },
    {
      key: 'prices',
      index: true,
      url: `${DOCS}/130055`,
      expect: 'html',
      extract: (html) => [...new Set([...tencentPriceRows(html).matchAll(/^\| ([A-Za-z][^|]*?) \|/gm)].map((m) => m[1]))],
      snapshot: (html) => tencentPriceRows(html),
    },
  ],
  acknowledged: [
    { pattern: /^(kl|vd)-video-/, reason: 'TokenHub took this old version offline on 2026-09-15 ("（2026-09-15下线）"); its token-billed successor is listed beside it' },
    ...skip('TokenHub retires it on 2026-11-24 ("Seedream-Image-v5.0-lite （2026-11-24 下线）")', ['seedream-image-v5.0-lite']),
    {
      pattern: /^(tripo-3d|hi3d|rodin-gen-3d)-/,
      reason: "skipped 2026-11-04: 3D through TokenHub, not built yet — litegen delivers a 3D job by reading the vendor's mesh after the vendor has billed, and a live run with a TokenHub key must first prove its files read",
    },
    ...skip('TokenHub retires it on 2026-11-12 ("Hy-Image-3.0 （2026-11-12 下线）"); its successor hy-image-v3.5-preview is carried', ['hy-image-v3']),
    ...skip('an old per-image version TokenHub took offline on 2026-09-15 ("（2026-09-15下线）")', ['hy-image-v3.0', 'hy-image-lite']),
    ...skip(
      'the model list\'s spelling of HY-Video-1.5; the API guide gives "model … 取值：hy-video-v1.5", which tokenhub/hy-video-v1.5 sends (an operator can swap it with model_mapping)',
      ['hy-video-1.5'],
    ),
    ...skip(
      'WAND-Vega image: its `size` lists no accepted values ("1K / 2K / 4K" and eight ratios only), the prompt limit is 8192 in the guide and 2000 in the overview, and 1.5-pro has no price',
      ['wand-vega-image-lite', 'wand-vega-image-flash', 'wand-vega-image-pro'],
    ),
    ...skip('template/app action (portrait driving, effect templates), not a general generation model', ['yt-video-humanactor', 'yt-video-fx']),
    ...skip('world model: a 360° panorama or a walkable 3DGS/mesh scene, not built yet', ['hy-world2-panorama', 'hy-world2-scene']),
    ...skip(
      'motion transfer (a character image plus a motion reference video by URL, "动作控制"): litegen carries no motion-control operation on any route yet',
      ['kling-video-motion-control-v3.0', 'kling-video-motion-control-v2.6'],
    ),
    ...skip(
      'Hunyuan 3D (POST /v1/api/3d/submit|query, priced in credits): not built yet — litegen delivers a 3D job by reading the vendor\'s mesh after the vendor has billed, and a live run with a TokenHub key must first prove its GLB reads (no Draco/meshopt) and that multi_view_images takes a data URI',
      ['hy-3d-3.0', 'hy-3d-3.1', 'hy-3d-express'],
    ),
    ...skip('a 3D tool on an existing mesh or rig (components, format, motion, retopology, rigging, texture, UV, low-poly), not built yet', [
      'hy-3d-component',
      'hy-3d-format',
      'hy-3d-motion',
      'hy-3d-retopology',
      'hy-3d-rigging',
      'hy-3d-texture',
      'hy-3d-uv',
      'hy-3d-polygen-mesh',
      'hy-3d-polygen-image',
    ]),
  ],
};
