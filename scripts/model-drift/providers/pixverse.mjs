// PixVerse — apidog docs: every API page has a `.md` twin that embeds the
// endpoint's OpenAPI spec as a ```yaml block, and llms.txt indexes the pages.
// The spec gives `model` no enum; its description is where the accepted
// values are listed ("now supports v3.5/v4/…/c1"), so ids are read from that
// one property of the three generate endpoints our adapter calls, never from
// the page at large (where "v2" is the API version). The YAML is read by
// extract.mjs's strict block-YAML reader (the drift core has no YAML dependency).
// Mapping mirrors litegen-core/src/providers/video/pixverse.rs (strip `pixverse/`).
import { markdownExcerpt, openapiExcerpt, parseYaml } from '../extract.mjs';

const DOCS = 'https://docs.platform.pixverse.ai';
// Model-version tokens: v3.5, v6, c1, v5-fast …
const MODEL_TOKEN = /\b[a-z]\d+(?:\.\d+)*(?:-[a-z0-9]+)*\b/gi;

// Both callbacks of a source parse the same body; parse each body once.
const docs = new Map();
const spec = (body) => {
  if (!docs.has(body)) docs.set(body, parseYaml(markdownExcerpt(body, { lang: 'yaml' })));
  return docs.get(body);
};

/** Accepted `model` values of one endpoint: the enum if PixVerse ever adds one, else those its description lists. */
function modelValues(body, path) {
  const content = spec(body).paths?.[path]?.post?.requestBody?.content ?? {};
  const model = Object.values(content)[0]?.schema?.properties?.model;
  if (!model) throw new Error(`POST ${path}: no request body \`model\` property — endpoint renamed or restructured`);
  if (Array.isArray(model.enum)) return model.enum;
  return String(model.description ?? '').match(MODEL_TOKEN) ?? [];
}

const endpoint = (key, slug, path) => ({
  key,
  url: `${DOCS}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => modelValues(body, path),
  snapshot: (_md, body, { ours }) => `# model values\n${modelValues(body, path).join('\n')}\n\n` + openapiExcerpt(spec(body), { paths: [path], ours }),
});

export default {
  provider: 'pixverse',
  models: {
    'pixverse/v4.5': 'v4.5',
    'pixverse/v5': 'v5',
    'pixverse/v3.5': 'v3.5',
  },
  sources: [
    endpoint('text-to-video', 'text-to-video-generation-13016634e0', '/openapi/v2/video/text/generate'),
    endpoint('image-to-video', 'image-to-video-generation-13016633e0', '/openapi/v2/video/img/generate'),
    endpoint('transition', 'transitionfirst-last-frame-generation-15123014e0', '/openapi/v2/video/transition/generate'),
    {
      // Model pages and video-generation endpoints: a new model or endpoint page shows up here.
      key: 'pages',
      index: true,
      url: `${DOCS}/llms.txt`,
      expect: 'text',
      extract: /^- (?:Models|API Reference > Video Generation) \[[^\]]*\]\(https:\/\/docs\.platform\.pixverse\.ai\/([\w-]+)\.md\)/gm,
    },
  ],
  acknowledged: [],
};
