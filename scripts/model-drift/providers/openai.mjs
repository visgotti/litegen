// OpenAI — two independent publications of one OpenAPI spec, both read through
// the request schemas of the endpoints the adapters call:
// - `openapi`: openai/openai-openapi, the published spec ("synchronized
//   automatically from upstream source"; default branch `main`). Parsed for
//   the typings excerpt.
// - `sdk-spec`: the transformed spec the official SDKs are generated from,
//   committed to openai-python as api_reference/openapi.transformed.yml (its
//   md5 is `openapi_transformed_spec_hash` in .castiron.stats.yml). It ships
//   with SDK releases, separately from the openai-openapi sync, so if that
//   sync stalls a new model still surfaces here. It replaces the
//   Stainless-hosted openapi.documented.yml, which 404s since OpenAI moved
//   SDK generation from Stainless to Castiron and deleted .stats.yml
//   (openai/openai-python#3599, 2026-08-12); the last Stainless copy is
//   frozen at 2026-07-31.
// Mapping mirrors litegen-core/src/providers/image/openai.rs (strip `openai/`).
// Sora 2 and the Videos API were shut down 2026-09-24 and left the catalog, so
// video/openai.rs serves no carried model and /videos is no longer read.
import { openapiModelIds, openapiExcerpt, parseYaml } from '../extract.mjs';

// image/openai.rs: POST /images/generations (JSON), POST /images/edits (multipart).
const ENDPOINTS = ['/images/generations', '/images/edits'];

export default {
  provider: 'openai',
  models: {
    'openai/gpt-image-2.5-sunburst': 'gpt-image-2.5-sunburst',
    'openai/gpt-image-2.5-flare': 'gpt-image-2.5-flare',
    'openai/gpt-image-2': 'gpt-image-2',
    'openai/gpt-image-1': 'gpt-image-1',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://raw.githubusercontent.com/openai/openai-openapi/main/openapi.json',
      expect: 'json',
      extract: (spec) => openapiModelIds(spec, { paths: ENDPOINTS }),
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
    {
      key: 'sdk-spec',
      url: 'https://raw.githubusercontent.com/openai/openai-python/main/api_reference/openapi.transformed.yml',
      expect: 'text',
      extract: (_text, body) => openapiModelIds(parseYaml(body), { paths: ENDPOINTS }),
    },
  ],
  acknowledged: [
    { pattern: /-\d{4}-\d{2}-\d{2}$/, reason: 'dated snapshot alias; the undated id is what we track' },
    { id: 'dall-e-2', reason: 'shut down upstream; the spec still lists it (see image/openai.rs)' },
    { id: 'dall-e-3', reason: 'shut down upstream; the spec still lists it (see image/openai.rs)' },
    ...['gpt-image-1.5', 'gpt-image-1-mini', 'chatgpt-image-latest'].map((id) => ({
      id,
      reason: 'skipped 2026-09-11: OpenAI shuts it down 2026-12-01 (developers.openai.com/api/docs/deprecations)',
    })),
  ],
};
