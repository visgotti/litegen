// MiniMax — Mintlify docs: every API page has a `.md` twin that embeds the
// endpoint's OpenAPI spec, and llms.txt indexes the pages.
// Mapping mirrors litegen-core/src/providers/{image,video}/minimax.rs (strip `minimax/`).
import { markdownExcerpt, yamlPrune } from '../extract.mjs';

const DOCS = 'https://platform.minimax.io/docs/api-reference';
const IDS = /\b(?:MiniMax-Hailuo-[\w.-]+|[TIS]2V-01[\w-]*|image-01[\w-]*)/g;
const spec = (_md, body) => yamlPrune(markdownExcerpt(body, { lang: 'yaml' }));
const page = (key, slug) => ({ key, url: `${DOCS}/${slug}.md`, expect: 'text', extract: IDS, snapshot: spec });

export default {
  provider: 'minimax',
  models: {
    'minimax/image-01': 'image-01',
    'minimax/MiniMax-Hailuo-02': 'MiniMax-Hailuo-02',
    'minimax/MiniMax-Hailuo-2.3': 'MiniMax-Hailuo-2.3',
    'minimax/T2V-01-Director': 'T2V-01-Director',
    'minimax/S2V-01': 'S2V-01',
  },
  sources: [
    page('text-to-image', 'image-generation-t2i'),
    page('image-to-image', 'image-generation-i2i'),
    page('text-to-video', 'video-generation-t2v'),
    page('image-to-video', 'video-generation-i2v'),
    page('first-last-frame', 'video-generation-fl2v'),
    page('subject-reference', 'video-generation-s2v'),
    {
      key: 'pages',
      index: true,
      url: 'https://platform.minimax.io/docs/llms.txt',
      expect: 'text',
      extract: /\/docs\/api-reference\/(?:image|video)[\w-]*\.md/g,
    },
  ],
  acknowledged: [],
};
