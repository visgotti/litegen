// Runway — the public OpenAPI spec the adapters were written against.
// Mapping mirrors litegen-core/src/providers/image/runway.rs (strip `runway/`)
// and video/runway.rs `resolve_model`.
import { openapiModelIds, openapiExcerpt } from '../extract.mjs';

// Endpoints the adapters call. Models on other endpoints (video_to_video,
// avatars, audio) need adapter work, so they surface through the endpoint
// list in the snapshot rather than as ids.
const ENDPOINTS = ['/v1/text_to_image', '/v1/image_to_video', '/v1/text_to_video'];

// Third-party models Runway resells on the same endpoints. Listed by exact id
// (not a pattern) so a new Runway-native model still surfaces as new.
const RESOLD =
  'skipped 2026-09-11: resold third-party model — carried direct from its own vendor, or reachable only ' +
  'through Runway and not adapter-ready (the Runway adapters map unknown ids to gen4_image / gen4_turbo); ' +
  'revisit Grok/Wan/Muse/HappyHorse as one batch if wanted';
const RESOLD_IDS = [
  'gemini_2.5_flash',
  'gemini_image3.1_flash',
  'gemini_image3_pro',
  'gemini_omni_flash',
  'gemini_omni_flash_1.1',
  'gpt_image_2',
  'gpt_image_2_5_flare',
  'gpt_image_2_5_sunburst',
  'grok_imagine_1_5',
  'grok_imagine_image_2',
  'h3_max',
  'hailuo3',
  'happyhorse_1_0',
  'muse_image',
  'seedance2',
  'seedance2_5',
  'seedance2_fast',
  'seedance2_mini',
  'seedream5_lite',
  'seedream5_pro',
  'veo3.1',
  'veo3.1_fast',
  'wan3',
  'wan3_prime',
];

export default {
  provider: 'runway',
  models: {
    'runway/gen4_image': 'gen4_image',
    'runway/gen4_image_turbo': 'gen4_image_turbo',
    'runway/gen4-turbo': 'gen4_turbo',
    'runway/gen4.5': 'gen4.5',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://docs.dev.runwayml.com/openapi.json',
      expect: 'json',
      extract: (spec) => openapiModelIds(spec, { paths: ENDPOINTS }),
      snapshot: (spec, _body, { ours }) =>
        `# image/video endpoints\n${Object.keys(spec.paths).filter((p) => /image|video/.test(p)).sort().join('\n')}\n\n` +
        openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
  ],
  acknowledged: RESOLD_IDS.map((id) => ({ id, reason: RESOLD })),
};
