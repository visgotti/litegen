// MiniMax — Mintlify docs: every API page has a `.md` twin that embeds the
// endpoint's OpenAPI spec as a ```yaml block, and llms.txt indexes the pages.
// Mapping mirrors litegen-core/src/providers/{image,video}/minimax.rs (strip `minimax/`).
// The adapters call POST /v1/image_generation (image-01: text-to-image, and
// image-to-image via subject_reference), POST /v1/video_generation (Hailuo:
// one page per mode) and POST /v2/video_generation (MiniMax-H3 / -H3-Max, the
// V2 API; the adapter routes only those ids there). The subject-reference page
// carries no catalog model since S2V-01 was retired (2026-11-16); it is still
// read because video/minimax.rs keeps S2V-01's request shape for an operator's
// runtime row, and a change to that page should still show.
// Every page is read the same way: ids from its embedded spec's request
// `model` enum — the values that endpoint accepts, whatever their spelling —
// and the snapshot is that spec with prose keys pruned.
import { markdownExcerpt, openapiModelIds, parseYaml, yamlPrune } from '../extract.mjs';

const DOCS = 'https://platform.minimax.io/docs/api-reference';
const page = (key, slug, path) => ({
  key,
  url: `${DOCS}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => openapiModelIds(parseYaml(markdownExcerpt(body, { lang: 'yaml' })), { paths: [path] }),
  snapshot: (_md, body) => yamlPrune(markdownExcerpt(body, { lang: 'yaml' })),
});

export default {
  provider: 'minimax',
  models: {
    'minimax/image-01': 'image-01',
    'minimax/MiniMax-Hailuo-02': 'MiniMax-Hailuo-02',
    'minimax/MiniMax-Hailuo-2.3': 'MiniMax-Hailuo-2.3',
    'minimax/MiniMax-Hailuo-2.3-Fast': 'MiniMax-Hailuo-2.3-Fast',
    'minimax/MiniMax-H3': 'MiniMax-H3',
    'minimax/MiniMax-H3-Max': 'MiniMax-H3-Max',
    'minimax/MiniMax-H3-Highspeed': 'MiniMax-H3-Highspeed',
    'minimax/MiniMax-H3-Lite': 'MiniMax-H3-Lite',
  },
  sources: [
    page('text-to-image', 'image-generation-t2i', '/v1/image_generation'),
    page('image-to-image', 'image-generation-i2i', '/v1/image_generation'),
    page('text-to-video', 'video-generation-t2v', '/v1/video_generation'),
    page('image-to-video', 'video-generation-i2v', '/v1/video_generation'),
    page('first-last-frame', 'video-generation-fl2v', '/v1/video_generation'),
    page('subject-reference', 'video-generation-s2v', '/v1/video_generation'),
    page('video-v2', 'video-generation-v2-create', '/v2/video_generation'),
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
    // Carried until 2026-11-16; still in MiniMax's t2v and s2v `model` enums.
    { id: 'T2V-01-Director', reason: 'retired 2026-11-16: MiniMax publishes no price for it (not even under "Legacy Models"), so litegen cannot bill it truthfully' },
    { id: 'S2V-01', reason: 'retired 2026-11-16: MiniMax publishes no price for it (not even under "Legacy Models"), so litegen cannot bill it truthfully' },
  ],
};
