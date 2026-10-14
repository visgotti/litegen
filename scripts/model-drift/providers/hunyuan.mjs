// Tencent Hunyuan (Tencent Cloud). Two kinds of source:
//
// 1. The official Go SDK on GitHub, generated from the TC3 API definitions:
//    client.go has one method per action, models.go the request/response
//    structs with the parameter docs as comments. These generation APIs take
//    no model name, so the "ids" are the async Submit…Job action names of the
//    services that host image/video generation:
//      hunyuan-client  hunyuan 2023-09-01 (Hunyuan image generation)
//      vclm-client     vclm 2024-05-23 (video generation: Hunyuan, Kling, Vidu…)
//      aiart-client    aiart 2022-12-29 (hosted SubmitTextToImageJob, Hunyuan
//                      Image 3.0, until Tencent deleted it on 2026-09-22)
//      vclm-models     the `Model` values SubmitImageToVideoJob accepts (it is
//                      a Kling-branded resale action we skip) + that request
//                      struct as the snapshot
//      vclm-hunyuan-video  index: the SubmitHunyuanToVideoJob request struct,
//                      the vclm `Image` struct it nests, and the
//                      DescribeHunyuanToVideoJob response struct (our video
//                      action's contract)
//      vclm-image-to-video-general  index: the same three for
//                      SubmitImageToVideoGeneralJob / DescribeImageToVideoGeneralJob
//                      (hunyuan-video-i2v's contract)
//      hunyuan-models  index: the SubmitHunyuanImageJob request struct, the
//                      hunyuan `Image` struct it nests (ContentImage) and the
//                      QueryHunyuanImageJob response struct
//
// 2. TokenHub (大模型服务平台 TokenHub), where Tencent is moving every Hunyuan
//    model: "迁移后，原平台将不再新增模型能力，并停止支持新购模型服务" (the
//    hunyuan / vclm / aiart billing pages, 2026). New Hunyuan models therefore
//    show up only here, as `model` values of a Bearer-key HTTP API, not as TC3
//    actions. Tencent's docs center server-renders these pages (Slate HTML),
//    and each has a 模型列表 table whose `model 参数值` column we compare:
//      tokenhub-hy-image  Hy 生图调用指南: hy-image-v3 (Hunyuan Image 3.0 —
//                      the model hunyuan/hunyuan-image-3 serves — now a
//                      synchronous TokenHub call) and its successors. The
//                      snapshot is the model list, the Hy-Image-3.0 endpoint
//                      and parameter tables, and the appendix (preset sizes,
//                      reference-image limits). It replaced the
//                      aiart-hunyuan-image-3 source (the SubmitTextToImageJob
//                      request struct), whose struct no longer exists.
//      tokenhub-hy-video  Hy 生视频调用指南: hy-video-v1.5 and its successors;
//                      snapshot the model list and the submit/query tables.
//
// Mapping mirrors litegen-core/src/providers/image/hunyuan.rs (action
// SubmitHunyuanImageJob on hunyuan, or SubmitTextToImageJob on aiart for
// hunyuan-image-3; no model field) and video/hunyuan.rs (action
// SubmitHunyuanToVideoJob, Tencent's native Hunyuan video model, or
// SubmitImageToVideoGeneralJob for hunyuan-video-i2v; no model field). Until
// 2026-09-11 hunyuan-video called SubmitImageToVideoJob with
// `Model: "Kling-V1-6"`, i.e. Kling 1.6 resold through Tencent Cloud.
//
// aiart release 31 (2026-09-22, https://cloud.tencent.com/document/product/1668/88078)
// deleted SubmitTextToImageJob and QueryTextToImageJob, the two actions the
// adapter calls for hunyuan-image-3, so that model reports `missing` until
// the adapter moves (TokenHub hy-image-v3) or the model is dropped.

const SDK = 'https://raw.githubusercontent.com/TencentCloud/tencentcloud-sdk-go/master/tencentcloud';
const TOKENHUB_DOCS = 'https://cloud.tencent.com/document/product/1823';

/** Acknowledge a list of upstream ids with one reason (ids compare case-insensitively). */
const skip = (reason, ids) => ids.map((id) => ({ id, reason: `skipped 2026-09-11: ${reason}` }));

/** Async generation actions of a client.go (`func (c *Client) SubmitXxxJob(request …`), minus model training. */
const submitJobs = (go) =>
  [...go.matchAll(/^func \(c \*Client\) (Submit\w+Job)\(request \*/gm)].map((m) => m[1]).filter((a) => !/Train/.test(a));

/** `type <name> struct { … }` of a models.go, verbatim. */
function goStruct(go, name) {
  const start = go.indexOf(`\ntype ${name} struct {`);
  if (start < 0) throw new Error(`struct ${name} not found — the action was renamed or removed upstream`);
  return go.slice(start + 1, go.indexOf('\n}', start) + 2) + '\n';
}

/** Several structs of a models.go, in order, blank-line separated. */
const goStructs = (go, names) => names.map((n) => goStruct(go, n)).join('\n');
/** An action's request struct plus the structs it references, headed by the action name the adapter sends. */
const actionStructs = (go, action, more) => `// action ${action}\n${goStructs(go, [`${action}RequestParams`, ...more])}`;

/** Top-level field names of a struct (the index id list of a contract source). */
const goFields = (go, name) => [...goStruct(go, name).matchAll(/^\t(\w+) /gm)].map((m) => m[1]);

/** Values listed in the `Model` field comment ("v1.6：Kling-V1-6<br>v2.0：Kling-V2-Master…"). */
function modelValues(go, struct) {
  const comment = goStruct(go, struct).match(/((?:\t\/\/[^\n]*\n)+)\tModel \*string/)?.[1];
  if (!comment) throw new Error(`${struct} has no documented Model field`);
  return [...comment.matchAll(/[：:]\s*([A-Za-z][\w.-]*)/g)].map((m) => m[1]);
}

// ─── Tencent docs center (TokenHub) pages ────────────────────────────────────

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

export default {
  provider: 'hunyuan',
  models: {
    'hunyuan/hunyuan-image': 'SubmitHunyuanImageJob',
    'hunyuan/hunyuan-image-3': 'SubmitTextToImageJob',
    'hunyuan/hunyuan-video': 'SubmitHunyuanToVideoJob',
    'hunyuan/hunyuan-video-i2v': 'SubmitImageToVideoGeneralJob',
  },
  sources: [
    { key: 'hunyuan-client', url: `${SDK}/hunyuan/v20230901/client.go`, expect: 'text', extract: submitJobs },
    { key: 'vclm-client', url: `${SDK}/vclm/v20240523/client.go`, expect: 'text', extract: submitJobs },
    { key: 'aiart-client', url: `${SDK}/aiart/v20221229/client.go`, expect: 'text', extract: submitJobs },
    {
      key: 'vclm-models',
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => modelValues(go, 'SubmitImageToVideoJobRequestParams'),
      snapshot: (go) => goStruct(go, 'SubmitImageToVideoJobRequestParams'),
    },
    {
      key: 'vclm-hunyuan-video',
      index: true,
      typings: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitHunyuanToVideoJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitHunyuanToVideoJob', ['Image', 'DescribeHunyuanToVideoJobResponseParams']),
    },
    {
      key: 'vclm-image-to-video-general',
      index: true,
      typings: true,
      url: `${SDK}/vclm/v20240523/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitImageToVideoGeneralJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitImageToVideoGeneralJob', ['Image', 'DescribeImageToVideoGeneralJobResponseParams']),
    },
    {
      key: 'hunyuan-models',
      index: true,
      typings: true,
      url: `${SDK}/hunyuan/v20230901/models.go`,
      expect: 'text',
      extract: (go) => goFields(go, 'SubmitHunyuanImageJobRequestParams'),
      snapshot: (go) => actionStructs(go, 'SubmitHunyuanImageJob', ['Image', 'QueryHunyuanImageJobResponseParams']),
    },
    {
      key: 'tokenhub-hy-image',
      url: `${TOKENHUB_DOCS}/135745`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^Hy-Image-3\.0 /, /^附录$/], drop: NOT_CONTRACT }),
    },
    {
      key: 'tokenhub-hy-video',
      url: `${TOKENHUB_DOCS}/137202`,
      expect: 'html',
      extract: (html) => tokenhubModelIds(html),
      snapshot: (html) => tokenhubExcerpt(html, { keep: [/^概述$/, /^视频生成$/, /^查询任务结果$/], drop: NOT_CONTRACT }),
    },
  ],
  acknowledged: [
    ...skip('Kling/Vidu resale action on vclm; we integrate Kling and Vidu directly', [
      'SubmitImageToVideoJob',
      'SubmitTextToVideoJob',
      'SubmitMotionControlKlingJob',
      'SubmitVideoEditKlingJob',
      'SubmitVideoExtendKlingJob',
      'SubmitImageToVideoViduJob',
      'SubmitReferenceToVideoViduJob',
      'SubmitTextToVideoViduJob',
    ]),
    // The `Model` values of SubmitImageToVideoJob (vclm-models source).
    ...skip('Kling model resold through SubmitImageToVideoJob; we integrate Kling directly', [
      'Kling-V1-6',
      'Kling-V2-Master',
      'Kling-V2-1',
      'Kling-V2-5-Turbo',
      'Kling-V2-6',
      'kling-v3',
    ]),
    ...skip('template/app action, not a general generation model', [
      'SubmitDrawPortraitJob',
      'SubmitGlamPicJob',
      'SubmitMemeJob',
      'SubmitHumanActorJob',
      'SubmitPortraitSingJob',
      'SubmitTemplateToVideoJob',
      'SubmitVideoFaceFusionJob',
    ]),
    ...skip('multi-turn chat variant of SubmitHunyuanImageJob (same model, needs a conversation-id workflow)', [
      'SubmitHunyuanImageChatJob',
    ]),
    ...skip('aiart text-to-image (advanced) action; the SDK marks it migrated to SubmitHunyuanImageJob', [
      'SubmitTextToImageProJob',
    ]),
  ],
};
