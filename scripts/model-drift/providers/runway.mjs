// Runway — the public OpenAPI spec the adapters were written against.
// Mapping mirrors litegen-core/src/providers/image/runway.rs (strip `runway/`)
// and video/runway.rs `resolve_model`.
import { openapiModelIds, openapiExcerpt } from '../extract.mjs';

// Endpoints the adapters call. Models on other endpoints (video_to_video,
// avatars, audio) need adapter work, so they surface through the endpoint
// list in the snapshot rather than as ids.
const ENDPOINTS = ['/v1/text_to_image', '/v1/image_to_video', '/v1/text_to_video'];

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
  acknowledged: [],
};
