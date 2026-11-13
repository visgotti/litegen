// Runway — the public OpenAPI spec the adapters were written against.
// Mapping mirrors litegen-core/src/providers/image/runway.rs (strip `runway/`)
// and video/runway.rs `resolve_model`.
import { openapiModelIds, openapiExcerpt } from '../extract.mjs';

// Endpoints the adapters call. Models on other endpoints (video_to_video,
// avatars, audio) need adapter work, so they surface through the endpoint
// list in the snapshot rather than as ids.
const ENDPOINTS = ['/v1/text_to_image', '/v1/image_to_video', '/v1/text_to_video'];

// The pricing guide's text up to its cost calculator (HTML and script): every
// price table, so a price change shows as a changed snapshot.
const priceText = (md) => md.slice(0, md.indexOf('# Cost calculator')).replace(/^---[\s\S]*?---\n/, '');

export default {
  provider: 'runway',
  // The resold models (image/runway.rs and video/runway.rs RESOLD) are
  // runway/<Runway's model>, carried under the hub-route policy (2026-11-10).
  models: {
    'runway/gen4_image': 'gen4_image',
    'runway/gen4_image_turbo': 'gen4_image_turbo',
    'runway/gen4-turbo': 'gen4_turbo',
    'runway/gen4.5': 'gen4.5',
    'runway/bria_fibo_1_5': 'bria_fibo_1_5',
    'runway/bria_fibo_edit_1_5': 'bria_fibo_edit_1_5',
    'runway/gemini_2.5_flash': 'gemini_2.5_flash',
    'runway/gemini_image3.1_flash': 'gemini_image3.1_flash',
    'runway/gemini_image3.1_flash_lite': 'gemini_image3.1_flash_lite',
    'runway/gemini_image3_pro': 'gemini_image3_pro',
    'runway/gpt_image_2': 'gpt_image_2',
    'runway/gpt_image_2_5_flare': 'gpt_image_2_5_flare',
    'runway/gpt_image_2_5_sunburst': 'gpt_image_2_5_sunburst',
    'runway/gpt_image_2_6_flare': 'gpt_image_2_6_flare',
    'runway/gpt_image_2_6_sunburst': 'gpt_image_2_6_sunburst',
    'runway/grok_imagine_image_2': 'grok_imagine_image_2',
    'runway/ideogram_4_0': 'ideogram_4_0',
    'runway/ideogram_4_5': 'ideogram_4_5',
    'runway/ideogram_vector': 'ideogram_vector',
    'runway/muse_image': 'muse_image',
    'runway/nano_banana_2.1': 'nano_banana_2.1',
    'runway/seedream5_flash': 'seedream5_flash',
    'runway/seedream5_lite': 'seedream5_lite',
    'runway/seedream5_pro': 'seedream5_pro',
    'runway/gemini_omni_flash': 'gemini_omni_flash',
    'runway/gemini_omni_flash_1.1': 'gemini_omni_flash_1.1',
    'runway/gemini_omni_flash_1.2': 'gemini_omni_flash_1.2',
    'runway/grok_imagine_1_5': 'grok_imagine_1_5',
    'runway/grok_imagine_1_5_lite': 'grok_imagine_1_5_lite',
    'runway/h3_max': 'h3_max',
    'runway/hailuo3': 'hailuo3',
    'runway/happyhorse_1_0': 'happyhorse_1_0',
    'runway/kling2.5_turbo_pro': 'kling2.5_turbo_pro',
    'runway/kling3.0_4k': 'kling3.0_4k',
    'runway/kling3.0_pro': 'kling3.0_pro',
    'runway/kling3.0_standard': 'kling3.0_standard',
    'runway/klingO3_4k': 'klingO3_4k',
    'runway/klingO3_pro': 'klingO3_pro',
    'runway/klingO3_standard': 'klingO3_standard',
    'runway/kling_4': 'kling_4',
    'runway/kling_4_flash': 'kling_4_flash',
    'runway/seedance2': 'seedance2',
    'runway/seedance2_5': 'seedance2_5',
    'runway/seedance2_fast': 'seedance2_fast',
    'runway/seedance2_mini': 'seedance2_mini',
    'runway/veo3.1': 'veo3.1',
    'runway/veo3.1_fast': 'veo3.1_fast',
    'runway/wan3': 'wan3',
    'runway/wan3_prime': 'wan3_prime',
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
    {
      key: 'pricing',
      url: 'https://docs.dev.runwayml.com/guides/pricing.md',
      expect: 'text',
      index: true,
      extract: (md) => [...new Set([...priceText(md).matchAll(/^\| `([\w.]+)`/gm)].map((m) => m[1]))],
      snapshot: (md) => priceText(md),
    },
  ],
  acknowledged: [
    ...['recraft_v4_1_flash', 'recraft_v4_1_vector'].map((id) => ({
      id,
      reason: 'skipped 2026-11-10: no price on the pricing guide (https://docs.dev.runwayml.com/guides/pricing.md)',
    })),
  ],
};
