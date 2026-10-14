// PixVerse — apidog docs: every API page has a `.md` twin that embeds the
// endpoint's OpenAPI spec as a ```yaml block, and llms.txt indexes the pages.
// The adapter (litegen-core/src/providers/video/pixverse.rs) calls three
// generate endpoints — text/, img/ (first frame) and transition/ (first +
// last frame) — and uploads reference images to image/upload first.
// The spec gives `model` no enum; its description is where the accepted
// values are listed ("now supports v3.5/v4/…/c1"), so ids are read from that
// one property of the three generate endpoints, never from the page at large
// (where "v2" is the API version). The per-model rules live in descriptions
// too ("v6/c1 : 1~15", "v6/c1 models : 16:9, …, 21:9", "Supported in
// v5.5/v5.6/v6/c1 models"), so each snapshot keeps the request properties'
// descriptions beside the pruned schema. The YAML is read by extract.mjs's
// strict block-YAML reader (the drift core has no YAML dependency).
// Mapping mirrors litegen-core/src/providers/video/pixverse.rs (strip `pixverse/`).
import { markdownExcerpt, openapiExcerpt, parseYaml } from '../extract.mjs';

const DOCS = 'https://docs.platform.pixverse.ai';
const UPLOAD = '/openapi/v2/image/upload';
// Model-version tokens: v3.5, v6, c1, v5-fast …
const MODEL_TOKEN = /\b[a-z]\d+(?:\.\d+)*(?:-[a-z0-9]+)*\b/gi;

// Both callbacks of a source parse the same body; parse each body once.
const memo = (parse) => {
  const seen = new Map();
  return (body) => {
    if (!seen.has(body)) seen.set(body, parse(body));
    return seen.get(body);
  };
};
const spec = memo((body) => parseYaml(markdownExcerpt(body, { lang: 'yaml' })));

/**
 * The upload page's responses use YAML anchors (`&ref_0`), which parseYaml
 * refuses, so only its request body is parsed — the part the adapter sends.
 */
const uploadSpec = memo((body) => {
  const yaml = markdownExcerpt(body, { lang: 'yaml' });
  if (!yaml.split('\n').some((l) => l.trim() === `${UPLOAD}:`)) throw new Error(`endpoint ${UPLOAD} is not in the spec — renamed or removed upstream`);
  const block = yaml.match(/^( +)requestBody:\n((?:\1 +.*\n|[ \t]*\n)*)/m);
  if (!block) throw new Error(`POST ${UPLOAD}: no requestBody — endpoint restructured`);
  return { paths: { [UPLOAD]: { post: { requestBody: parseYaml(block[2]) } } } };
});

/** The request body schema's properties of one endpoint. */
function requestProperties(doc, path) {
  const content = doc.paths?.[path]?.post?.requestBody?.content ?? {};
  const properties = Object.values(content)[0]?.schema?.properties;
  if (!properties) throw new Error(`POST ${path}: no request body properties — endpoint renamed or restructured`);
  return properties;
}

/** Accepted `model` values of one endpoint: the enum if PixVerse ever adds one, else those its description lists. */
function modelValues(body, path) {
  const model = requestProperties(spec(body), path).model;
  if (!model) throw new Error(`POST ${path}: no request body \`model\` property — endpoint renamed or restructured`);
  if (Array.isArray(model.enum)) return model.enum;
  return String(model.description ?? '').match(MODEL_TOKEN) ?? [];
}

/** Each request property's description, whitespace-collapsed: PixVerse's per-model rules. */
const descriptions = (doc, path) =>
  Object.entries(requestProperties(doc, path))
    .filter(([, p]) => p?.description)
    .map(([name, p]) => `${name}: ${String(p.description).replace(/\s+/g, ' ').trim()}`)
    .join('\n');

const typings = (doc, path, ours) =>
  `# request property descriptions\n${descriptions(doc, path)}\n\n` + openapiExcerpt(doc, { paths: [path], ours });

const endpoint = (key, slug, path) => ({
  key,
  url: `${DOCS}/${slug}.md`,
  expect: 'text',
  extract: (_md, body) => modelValues(body, path),
  snapshot: (_md, body, { ours }) => `# model values\n${modelValues(body, path).join('\n')}\n\n` + typings(spec(body), path, ours),
});

export default {
  provider: 'pixverse',
  models: {
    'pixverse/v4.5': 'v4.5',
    'pixverse/v5': 'v5',
    'pixverse/v3.5': 'v3.5',
    'pixverse/v6': 'v6',
    'pixverse/c1': 'c1',
  },
  sources: [
    endpoint('text-to-video', 'text-to-video-generation-13016634e0', '/openapi/v2/video/text/generate'),
    endpoint('image-to-video', 'image-to-video-generation-13016633e0', '/openapi/v2/video/img/generate'),
    endpoint('transition', 'transitionfirst-last-frame-generation-15123014e0', '/openapi/v2/video/transition/generate'),
    {
      // Reference-image upload (no model): the endpoint, so a rename fails here.
      key: 'upload-image',
      index: true,
      url: `${DOCS}/upload-image-13016631e0.md`,
      expect: 'text',
      extract: (_md, body) => Object.keys(uploadSpec(body).paths),
      snapshot: (_md, body) => typings(uploadSpec(body), UPLOAD, []),
    },
    {
      // Model pages and video-generation endpoints: a new model or endpoint page shows up here.
      key: 'pages',
      index: true,
      url: `${DOCS}/llms.txt`,
      expect: 'text',
      extract: /^- (?:Models|API Reference > Video Generation) \[[^\]]*\]\(https:\/\/docs\.platform\.pixverse\.ai\/([\w-]+)\.md\)/gm,
    },
  ],
  acknowledged: [
    { id: 'v5.5', reason: 'skipped 2026-09-11: legacy; v4.5/v5 carried, v6/c1 added' },
    { id: 'v5.6', reason: 'skipped 2026-09-11: legacy; v4.5/v5 carried, v6/c1 added' },
    { id: 'v4', reason: 'skipped 2026-09-11: legacy; v4.5/v5 carried, v6/c1 added' },
  ],
};
