// AWS Bedrock — docs.aws.amazon.com serves a markdown twin of every page
// (`<page>.md`, linked as "View a markdown version of this page"). The old
// supported-models table is gone; model ids now live on per-model cards and
// provider pages, so the sources are:
//   nova-canvas      the Nova Canvas model card (Programmatic Access model id,
//                    lifecycle status and EOL date)
//   nova-reel        the Nova user guide page the video adapter was written
//                    against: the StartAsyncInvoke `modelId` (the Bedrock
//                    card lists only the older amazon.nova-reel-v1:0) and the
//                    request structures
//   nova-canvas-api  index: Nova Canvas request structures, one per taskType
//   lifecycle        index: the Legacy/EOL table rows for our models
//   stability, luma  the other image/video models Bedrock hosts
//   cards            index: image/video model cards in "Models at a glance",
//                    so a new card (e.g. a Nova Canvas successor) shows up
// Mapping mirrors litegen-core/src/providers/image/bedrock.rs (InvokeModel URL
// path) and video/bedrock.rs (StartAsyncInvoke body `modelId`): strip `bedrock/`.
import { markdownExcerpt } from '../extract.mjs';

const GUIDE = 'https://docs.aws.amazon.com/bedrock/latest/userguide';
const NOVA = 'https://docs.aws.amazon.com/nova/latest/userguide';

/** Lifecycle bullets (`+ **Model EOL date:** …`) of a model card. */
const lifecycleBullets = (md) => {
  const lines = md.split('\n').filter((l) => /^\+ \*\*(Model launch date|EOL no sooner than|Legacy period|Model EOL date|Model lifecycle):\*\*/.test(l));
  if (lines.length === 0) throw new Error('no lifecycle bullets on the model card — the page was restructured');
  return lines.map((l) => l.replace(/\s+$/, ''));
};

/** Rows of the card's Programmatic Access table (endpoint, model id, inference ids). */
const programmaticRows = (md) => md.split('\n').filter((l) => /^\|\s*bedrock-(runtime|mantle)\s*\|/.test(l)).map((l) => l.trim());

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

export default {
  provider: 'bedrock',
  models: {
    'bedrock/amazon.nova-canvas-v1:0': 'amazon.nova-canvas-v1:0',
    'bedrock/amazon.nova-reel-v1:1': 'amazon.nova-reel-v1:1',
  },
  sources: [
    {
      key: 'nova-canvas',
      url: `${GUIDE}/model-card-amazon-nova-canvas.md`,
      expect: 'text',
      extract: /\bamazon\.nova-[a-z0-9-]+-v\d+:\d+/g,
      snapshot: (md) => [...lifecycleBullets(md), '', ...programmaticRows(md)].join('\n') + '\n',
    },
    {
      key: 'nova-reel',
      url: `${NOVA}/video-gen-access.md`,
      expect: 'text',
      extract: /\bamazon\.nova-[a-z0-9-]+-v\d+:\d+/g,
      // Parameter bullets (`+ **name** (Required) – constraint`) and the request structures.
      snapshot: (md) =>
        `# parameters\n${md
          .split('\n')
          .filter((l) => /^\+ \*\*\w+\*\*\s*\((Required|Optional)\)/.test(l))
          .map((l) => l.trim())
          .join('\n')}\n\n# request structures\n${markdownExcerpt(md)}`,
    },
    {
      key: 'nova-canvas-api',
      index: true,
      url: `${NOVA}/image-gen-req-resp-structure.md`,
      expect: 'text',
      extract: /"taskType":\s*"([A-Z_]+)"/g,
      snapshot: (md) => markdownExcerpt(md),
    },
    {
      key: 'lifecycle',
      index: true,
      url: `${GUIDE}/model-lifecycle-legacy.md`,
      expect: 'text',
      extract: /\*\*Model ID:\*\*\s*([\w.:-]+)/g,
      // One bullet group per model (provider, name, id, regions, dates); keep ours.
      snapshot: (md, _body, { ours }) => {
        const want = new Set(ours.map((s) => s.toLowerCase()));
        const groups = md.split(/\n(?=- \*\*)/).filter((g) => want.has((g.match(/\*\*Model ID:\*\*\s*([\w.:-]+)/)?.[1] ?? '').toLowerCase()));
        return groups.length
          ? groups.map((g) => g.split('\n').filter((l) => /^\s*- \*\*/.test(l)).map((l) => l.trim()).join('\n')).join('\n\n') + '\n'
          : '(none of our models is Legacy or pending EOL)\n';
      },
    },
    {
      key: 'stability',
      url: `${GUIDE}/stable-image-services.md`,
      expect: 'text',
      // Samples call cross-Region inference profiles (us.stability.…); the base model id follows the prefix.
      extract: /\b(stability\.[a-z0-9-]+-v\d+:\d+)/g,
    },
    {
      key: 'luma',
      url: `${GUIDE}/model-parameters-luma.md`,
      expect: 'text',
      extract: /\b(luma\.[a-z0-9-]+-v\d+:\d+)/g,
    },
    {
      key: 'cards',
      index: true,
      url: `${GUIDE}/model-cards.md`,
      expect: 'text',
      extract: /\((model-card-(?:amazon-[\w-]*(?:canvas|reel|image|video|omni)[\w-]*|stability-ai-[\w-]+|luma-[\w-]+))\.md\)/g,
    },
  ],
  acknowledged: [
    { id: 'luma.ray-v2:0', reason: 'skipped 2026-09-11: Luma Ray 2 is carried direct from Luma (models/luma.yaml)' },
    ...STABILITY_TOOL_IDS.map((id) => ({ id, reason: STABILITY_TOOLS })),
  ],
};
