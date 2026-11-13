// AWS Bedrock — docs.aws.amazon.com serves a markdown twin of every page
// (`<page>.md`, linked as "View a markdown version of this page"). The old
// supported-models table is gone; model ids now live on per-model cards and
// provider pages.
//
// We carry Stability AI's three text-to-image models on Bedrock (since
// 2026-11-04) and Luma Ray 2 (since 2026-11-10, beside luma/ray-2 under the
// hub-route policy); models/bedrock.yaml. Nova Canvas and Nova Reel reached
// end of life on 2026-09-30 and left the catalog (their sources went with
// them). The sources are:
//   lifecycle        index: our models' Legacy/EOL rows, under the section
//                    they sit in (scheduled EOL vs reached EOL)
//   stability        Bedrock's Stable Image services (edit/upscale tools)
//   stability-*      the cards of the Stability text-to-image models Bedrock
//                    hosts (their ids are on no shared page): lifecycle and
//                    the In-Region endpoint row
//   params-*         each model's request/response page (the typings: its
//                    parameter bullets, which the adapter's body follows)
//   luma             the Luma video model Bedrock hosts: its request
//                    fields (the typings video/bedrock.rs follows)
//   luma-ray2        Ray 2's card: lifecycle and the In-Region endpoint row
//   cards            index: image/video model cards in "Models at a glance",
//                    so a new card (e.g. a Nova Canvas successor) shows up
// Mapping mirrors litegen-core/src/providers/image/bedrock.rs (InvokeModel URL
// path, `stability_model`) and video/bedrock.rs (`resolve_model`, the async
// invoke's modelId): strip `bedrock/`.

const GUIDE = 'https://docs.aws.amazon.com/bedrock/latest/userguide';

/** Lifecycle bullets (`+ **Model EOL date:** …`) of a model card. */
const lifecycleBullets = (md) => {
  const lines = md.split('\n').filter((l) => /^\+ \*\*(Model launch date|EOL no sooner than|Legacy period|Model EOL date|Model lifecycle):\*\*/.test(l));
  if (lines.length === 0) throw new Error('no lifecycle bullets on the model card — the page was restructured');
  return lines.map((l) => l.replace(/\s+$/, ''));
};

/** Rows of the card's Programmatic Access table (endpoint, model id, inference ids). */
const programmaticRows = (md) => md.split('\n').filter((l) => /^\|\s*bedrock-(runtime|mantle)\s*\|/.test(l)).map((l) => l.trim());

/** A model card: its Programmatic Access model ids, and its lifecycle bullets + those rows as the snapshot. */
const card = (key, slug) => ({
  key,
  url: `${GUIDE}/model-card-${slug}.md`,
  expect: 'text',
  extract: (md) => {
    const rows = programmaticRows(md);
    if (rows.length === 0) throw new Error('no Programmatic Access row (| bedrock-runtime | <model id> | …) on the model card');
    return rows.map((r) => r.split('|')[2].trim());
  },
  snapshot: (md) => [...lifecycleBullets(md), '', ...programmaticRows(md)].join('\n') + '\n',
});

// The lifecycle page files each model under a bold section paragraph:
// "**Models with a scheduled EOL date**" (a table) or "**Models that have
// reached EOL**" (bullet groups). The section is the signal — a model moving
// from scheduled to reached-EOL keeps the same row text — so the snapshot
// keeps each of our rows/groups under its section heading.
const LIFECYCLE_ID = /\*\*Model ID:\*\*\s*([\w.:-]+)/;
const TABLE_ID = /^[a-z0-9-]+\.[\w.-]+-v\d+(?::\d+)?$/;
const cells = (row) => row.split('|').slice(1, -1).map((c) => c.trim());

function lifecycleSections(md) {
  const sections = [];
  for (const line of md.split('\n')) {
    const head = line.match(/^\*\*(Models [^*]+)\*\*\s*$/);
    if (head) sections.push({ title: head[1].trim(), lines: [] });
    else sections.at(-1)?.lines.push(line);
  }
  if (sections.length === 0) throw new Error('no "**Models …**" section paragraphs — the lifecycle page was restructured');
  return sections;
}

function lifecycleIds(md) {
  const ids = [...md.matchAll(new RegExp(LIFECYCLE_ID.source, 'g'))].map((m) => m[1]);
  for (const line of md.split('\n')) if (line.startsWith('|')) ids.push(...cells(line).filter((c) => TABLE_ID.test(c)));
  return ids;
}

function lifecycleSnapshot(md, ours) {
  const want = new Set(ours.map((s) => s.toLowerCase()));
  const out = [];
  for (const { title, lines } of lifecycleSections(md)) {
    const table = lines.filter((l) => l.startsWith('|'));
    const rows = table.filter((l) => cells(l).some((c) => want.has(c.toLowerCase())));
    const groups = lines
      .join('\n')
      .split(/\n(?=- \*\*)/)
      .filter((g) => want.has((g.match(LIFECYCLE_ID)?.[1] ?? '').toLowerCase()))
      .map((g) => g.split('\n').filter((l) => /^\s*- \*\*/.test(l)).map((l) => l.trim()).join('\n'));
    if (rows.length === 0 && groups.length === 0) continue;
    out.push(`## ${title}`, ...(rows.length ? [table[0].trim(), ...rows.map((r) => r.trim()), ''] : []), ...groups.flatMap((g) => [g, '']));
  }
  return out.length ? out.join('\n').replace(/\n+$/, '') + '\n' : '(none of our models is Legacy or pending EOL)\n';
}

// Bedrock's Stable Image services: edit/upscale tools that each need an input
// image (and some a mask), not text-to-image. Exact ids, so a new Stability
// text-to-image model on Bedrock still surfaces as new.
// @see https://docs.aws.amazon.com/bedrock/latest/userguide/stable-image-services.md
const STABILITY_TOOLS = 'skipped 2026-09-11: Stable Image service — an edit/upscale tool needing an input image; not adapter-ready';
const STABILITY_TOOL_IDS = [
  'stability.stable-conservative-upscale-v1:0',
  'stability.stable-creative-upscale-v1:0',
  'stability.stable-fast-upscale-v1:0',
  'stability.stable-image-control-sketch-v1:0',
  'stability.stable-image-control-structure-v1:0',
  'stability.stable-image-erase-object-v1:0',
  'stability.stable-image-inpaint-v1:0',
  'stability.stable-image-remove-background-v1:0',
  'stability.stable-image-search-recolor-v1:0',
  'stability.stable-image-search-replace-v1:0',
  'stability.stable-image-style-guide-v1:0',
  'stability.stable-outpaint-v1:0',
  'stability.stable-style-transfer-v1:0',
];

/** A Stability request/response page: the model ids in its samples, and its parameter bullets as the snapshot. */
const params = (key, slug) => ({
  key,
  url: `${GUIDE}/${slug}.md`,
  expect: 'text',
  extract: /\b(stability\.[a-z0-9-]+-v\d+:\d+)/g,
  snapshot: (md) => {
    const lines = md.split('\n').filter((l) => /^\s*\+ \*\*[\w\\]+\*\*/.test(l) || /^\s*\*\*[\w\\]+\*\* \(Required\)/.test(l)).map((l) => l.trim());
    if (lines.length === 0) throw new Error('no parameter bullets (+ **name** …) — the page was restructured');
    return lines.join('\n') + '\n';
  },
});

export default {
  provider: 'bedrock',
  models: {
    'bedrock/stability.sd3-5-large-v1:0': 'stability.sd3-5-large-v1:0',
    'bedrock/stability.stable-image-core-v1:1': 'stability.stable-image-core-v1:1',
    'bedrock/stability.stable-image-ultra-v1:1': 'stability.stable-image-ultra-v1:1',
    'bedrock/luma.ray-v2:0': 'luma.ray-v2:0',
  },
  sources: [
    {
      key: 'lifecycle',
      index: true,
      url: `${GUIDE}/model-lifecycle-legacy.md`,
      expect: 'text',
      extract: lifecycleIds,
      snapshot: (md, _body, { ours }) => lifecycleSnapshot(md, ours),
    },
    {
      key: 'stability',
      url: `${GUIDE}/stable-image-services.md`,
      expect: 'text',
      // Samples call cross-Region inference profiles (us.stability.…); the base model id follows the prefix.
      extract: /\b(stability\.[a-z0-9-]+-v\d+:\d+)/g,
    },
    // The Stability text-to-image models Bedrock hosts (InvokeModel, a body
    // unlike Nova Canvas's): Stable Image Services above lists only the tools.
    // @see https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-stability-diffusion.md
    card('stability-sd3-5-large', 'stability-ai-stable-diffusion-3-5-large'),
    card('stability-image-core', 'stability-ai-stable-image-core-1-0'),
    card('stability-image-ultra', 'stability-ai-stable-image-ultra-1-0'),
    params('params-sd3-5-large', 'model-parameters-diffusion-3-5-large'),
    params('params-image-core', 'model-parameters-diffusion-stable-image-core-text-image-request-response'),
    params('params-image-ultra', 'model-parameters-diffusion-stable-ultra-text-image-request-response'),
    // Luma Ray 2's request fields: the `**Fields**` bullets and the keyframe
    // bullets ("+ **prompt** – (string) …", "+ **frame0** – …").
    { ...params('luma', 'model-parameters-luma'), extract: /\b(luma\.[a-z0-9-]+-v\d+:\d+)/g },
    card('luma-ray2', 'luma-ai-ray2'),
    {
      key: 'cards',
      index: true,
      url: `${GUIDE}/model-cards.md`,
      expect: 'text',
      extract: /\((model-card-(?:amazon-[\w-]*(?:canvas|reel|image|video|omni)[\w-]*|stability-ai-[\w-]+|luma-[\w-]+))\.md\)/g,
    },
  ],
  acknowledged: [
    ...['amazon.nova-canvas-v1:0', 'amazon.nova-reel-v1:0', 'amazon.nova-reel-v1:1'].map((id) => ({
      id,
      reason: 'Bedrock EOL 2026-09-30 ("Models that have reached EOL"); removed from the catalog 2026-10-12',
    })),
    ...STABILITY_TOOL_IDS.map((id) => ({ id, reason: STABILITY_TOOLS })),
  ],
};
