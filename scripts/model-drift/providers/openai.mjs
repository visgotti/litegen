// OpenAI — the published OpenAPI spec (openai/openai-openapi). The GitHub JSON
// is parsed for the typings excerpt; the Stainless-hosted YAML (what the
// openai-openapi README calls canonical) is a regex cross-check, so a stale
// GitHub copy cannot hide a new model.
// Mapping mirrors litegen-core/src/providers/image/openai.rs (strip `openai/`)
// and video/openai.rs (table on the full id).
import { openapiModelIds, openapiExcerpt } from '../extract.mjs';

const ENDPOINTS = ['/images/generations', '/images/edits', '/videos'];

export default {
  provider: 'openai',
  models: {
    'openai/gpt-image-2': 'gpt-image-2',
    'openai/gpt-image-1': 'gpt-image-1',
    'openai/sora': 'sora-2',
    'openai/sora-2-pro': 'sora-2-pro',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.json',
      expect: 'json',
      extract: (spec) => openapiModelIds(spec, { paths: ENDPOINTS }),
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
    {
      key: 'stainless-spec',
      url: 'https://app.stainless.com/api/spec/documented/openai/openapi.documented.yml',
      expect: 'text',
      extract: /\b(?:gpt-image-\d[\w.-]*|dall-e-\d|sora-\d[\w-]*)/g,
    },
  ],
  acknowledged: [
    { pattern: /-\d{4}-\d{2}-\d{2}$/, reason: 'dated snapshot alias; the undated id is what we track' },
    { id: 'dall-e-2', reason: 'shut down upstream; the spec still lists it (see image/openai.rs)' },
    { id: 'dall-e-3', reason: 'shut down upstream; the spec still lists it (see image/openai.rs)' },
  ],
};
