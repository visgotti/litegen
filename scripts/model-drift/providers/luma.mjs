// Luma — Dream Machine API (api.lumalabs.ai/dream-machine/v1). Its ReadMe-style
// reference pages have `.md` twins that embed each endpoint's OpenAPI
// definition as a ```json block (docs.lumalabs.ai/openapi.json itself 404s):
// one page per endpoint our adapters call, plus llms.txt as the page index.
// The reference now points to a separate, newer "Luma Agents API"
// (docs.agents.lumalabs.ai, its own base URL and keys; uni-1, ray-3.2) that
// our adapters do not call — its model table is an index source, so a lineup
// change there shows up as a changed snapshot rather than as ids.
// Mapping mirrors litegen-core/src/providers/image/luma.rs and
// video/luma.rs `resolve_model` (luma/dream-machine is an alias: it sends ray-2).
import { markdownExcerpt, openapiModelIds, openapiExcerpt } from '../extract.mjs';

const REF = 'https://docs.lumalabs.ai/reference';
const embedded = (body) => JSON.parse(markdownExcerpt(body, { lang: 'json' }));
const reference = (key, slug, path) => ({
  key,
  url: `${REF}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => openapiModelIds(embedded(body), { paths: [path] }),
  snapshot: (_md, body, { ours }) => openapiExcerpt(embedded(body), { paths: [path], ours }),
});

export default {
  provider: 'luma',
  models: {
    'luma/photon-1': 'photon-1',
    'luma/photon-flash-1': 'photon-flash-1',
    'luma/dream-machine': 'ray-2',
    'luma/ray-2': 'ray-2',
    'luma/ray-3': 'ray-3',
    'luma/ray-flash-2': 'ray-flash-2',
    'luma/ray-hdr-3': 'ray-hdr-3',
  },
  sources: [
    // The guide posts to /generations; the reference names the same operation /generations/video.
    reference('create-generation', 'creategeneration', '/generations/video'),
    reference('generate-image', 'generateimage', '/generations/image'),
    {
      key: 'pages',
      index: true,
      url: 'https://docs.lumalabs.ai/llms.txt',
      expect: 'text',
      extract: /\/reference\/[\w-]+\.md/g,
    },
    {
      key: 'agents-api-models',
      index: true,
      url: 'https://docs.agents.lumalabs.ai/guides/model/index.md',
      expect: 'text',
      extract: (_md, body) => {
        const at = body.search(/^## At a glance[ \t]*$/m);
        if (at < 0) throw new Error('no "## At a glance" model table');
        const table = body.slice(at).split(/^## (?!At a glance)/m)[0];
        return [...table.matchAll(/^\|\s*`([^`]+)`\s*\|/gm)].map((m) => m[1]);
      },
    },
  ],
  acknowledged: [],
};
