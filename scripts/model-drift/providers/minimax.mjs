// MiniMax — Mintlify docs: every API page has a `.md` twin that embeds the
// endpoint's OpenAPI spec, and llms.txt indexes the pages.
// Mapping mirrors litegen-core/src/providers/{image,video}/minimax.rs (strip `minimax/`).
// The v1 pages are read with a regex over the page; the V2 create page (the
// H3 family's only endpoint) from its embedded spec's request `model` enum —
// the list of values that endpoint accepts.
import { markdownExcerpt, openapiModelIds, parseYaml, yamlPrune } from '../extract.mjs';

const DOCS = 'https://platform.minimax.io/docs/api-reference';
const IDS = /\b(?:MiniMax-Hailuo-[\w.-]+|[TIS]2V-01[\w-]*|image-01[\w-]*)/g;
const spec = (_md, body) => yamlPrune(markdownExcerpt(body, { lang: 'yaml' }));
const page = (key, slug) => ({ key, url: `${DOCS}/${slug}.md`, expect: 'text', extract: IDS, snapshot: spec });
// V2: POST /v2/video_generation's `model` enum (MiniMax-H3, MiniMax-H3-Max).
const v2Page = (key, slug, path) => ({
  key,
  url: `${DOCS}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => openapiModelIds(parseYaml(markdownExcerpt(body, { lang: 'yaml' })), { paths: [path] }),
  snapshot: spec,
});

export default {
  provider: 'minimax',
  models: {
    'minimax/image-01': 'image-01',
    'minimax/MiniMax-Hailuo-02': 'MiniMax-Hailuo-02',
    'minimax/MiniMax-Hailuo-2.3': 'MiniMax-Hailuo-2.3',
    'minimax/MiniMax-Hailuo-2.3-Fast': 'MiniMax-Hailuo-2.3-Fast',
    'minimax/T2V-01-Director': 'T2V-01-Director',
    'minimax/S2V-01': 'S2V-01',
    'minimax/MiniMax-H3': 'MiniMax-H3',
    'minimax/MiniMax-H3-Max': 'MiniMax-H3-Max',
  },
  sources: [
    page('text-to-image', 'image-generation-t2i'),
    page('image-to-image', 'image-generation-i2i'),
    page('text-to-video', 'video-generation-t2v'),
    page('image-to-video', 'video-generation-i2v'),
    page('first-last-frame', 'video-generation-fl2v'),
    page('subject-reference', 'video-generation-s2v'),
    v2Page('video-v2', 'video-generation-v2-create', '/v2/video_generation'),
    {
      key: 'pages',
      index: true,
      url: 'https://platform.minimax.io/docs/llms.txt',
      expect: 'text',
      extract: /\/docs\/api-reference\/(?:image|video)[\w-]*\.md/g,
    },
  ],
  acknowledged: [
    { id: 'T2V-01', reason: 'skipped 2026-09-11: first generation (*-01), superseded and unpriced' },
    { id: 'I2V-01', reason: 'skipped 2026-09-11: first generation (*-01), superseded and unpriced' },
    { id: 'I2V-01-Director', reason: 'skipped 2026-09-11: first generation (*-01), superseded and unpriced' },
    { id: 'I2V-01-live', reason: 'skipped 2026-09-11: first generation (*-01), superseded and unpriced' },
    { id: 'image-01-live', reason: 'skipped 2026-09-11: its difference from image-01 (carried) is undocumented' },
  ],
};
