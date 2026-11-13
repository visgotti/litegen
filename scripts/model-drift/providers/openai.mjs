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
//   frozen at 2026-07-31. Since 2026-11 the file writes one long path as a
//   YAML explicit key (`? /organization/workload_identity_providers/…`),
//   which parseYaml reads; api_reference/ holds no JSON twin of it.
// Mapping mirrors litegen-core/src/providers/image/openai.rs (strip `openai/`).
// Sora 2 and the Videos API were shut down 2026-09-24 and left the catalog, so
// video/openai.rs serves no carried model and /videos is no longer read.
import { openapiModelIds, openapiExcerpt, parseYaml, shutDownIds, allStrings } from '../extract.mjs';

// image/openai.rs: POST /images/generations (JSON), POST /images/edits (multipart).
const ENDPOINTS = ['/images/generations', '/images/edits'];

// The `model` enums keep ids long shut down (dall-e-2, dall-e-3, gpt-image-1);
// the spec's own prose says so ("`gpt-image-1` was shut down on October 23,
// 2026"), and such an id is not live: a carried one reports `missing`.
const liveModelIds = (doc) => {
  const dead = new Set(allStrings(doc).flatMap(shutDownIds));
  return openapiModelIds(doc, { paths: ENDPOINTS }).filter((id) => !dead.has(id));
};

export default {
  provider: 'openai',
  models: {
    'openai/gpt-image-2.5-sunburst': 'gpt-image-2.5-sunburst',
    'openai/gpt-image-2.5-flare': 'gpt-image-2.5-flare',
    'openai/gpt-image-2': 'gpt-image-2',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://raw.githubusercontent.com/openai/openai-openapi/main/openapi.json',
      timeoutMs: 180_000, // over 1 MB; raw.githubusercontent.com can be slow
      expect: 'json',
      extract: (spec) => liveModelIds(spec),
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
    {
      key: 'sdk-spec',
      url: 'https://raw.githubusercontent.com/openai/openai-python/main/api_reference/openapi.transformed.yml',
      timeoutMs: 180_000, // over 1 MB; raw.githubusercontent.com can be slow
      expect: 'text',
      extract: (_text, body) => liveModelIds(parseYaml(body)),
    },
    {
      // An announced shutdown reads as `changed`: the snapshot is the
      // deprecation rows that name a model we carry (audit D4, 2026-10-30).
      key: 'lifecycle',
      url: 'https://developers.openai.com/api/docs/deprecations.md',
      expect: 'text',
      index: true,
      extract: (md) => md.split('\n').filter((l) => l.startsWith('|')).flatMap((l) => [...l.matchAll(/`([^`\s]+)`/g)].map((m) => m[1])),
      snapshot: (md, _body, { ours }) => {
        // The "Model / system" cell, the second: a carried model named only as
        // the replacement is not being deprecated.
        const modelCell = (l) => l.split(/(?<!\\)\|/)[2] ?? '';
        const rows = md.split('\n').filter((l) => l.startsWith('|') && ours.some((id) => modelCell(l).includes(`\`${id}\``)));
        return rows.length ? rows.join('\n') : '(no deprecation row names a model we carry)';
      },
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
