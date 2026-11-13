// Stability AI — the API reference moved to docs.stability.ai (a Mintlify site)
// in 2026-10: platform.stability.ai/docs/api-reference is now a stub ("The
// Stability AI API reference has moved to https://docs.stability.ai/api-reference"),
// so the old content-hashed docs bundle is gone. Sources (re-read 2026-10-30):
//  - openapi: https://api.stability.ai/v2alpha/openapi — despite the path it is
//    the "v2beta" spec with the Stable Image generate routes (sd3 / core /
//    ultra). (docs.stability.ai/openapi.json served the same schemas until it
//    went behind the docs login, below.)
//  - v1-text-to-image, v1-image-to-image, v1-masking, v1-engines: the
//    "Version 1" API (SDXL 1.0, /v1/generation/{engine_id}/…), read from the
//    markdown twins of its four reference pages (V1_PAGES below). Each embeds
//    its operation as a one-path OpenAPI spec (`info.version: v1`) in a
//    ````yaml fence. The whole spec, docs.stability.ai/openapi-v1.json, which
//    llms.txt still lists under "OpenAPI Specs", 302s to
//    platform.stability.ai/docs-login since 2026-11, and
//    api.stability.ai/v1/openapi answers 401 (both checked 2026-11-09).
//  - release-notes: index: the markdown twin of the release notes, where
//    Stability announces deprecations (SD3.0 on 2025-04-17; SD 1.6 and Stable
//    Video on 2025-07-24). Each `**date**` line and the title under it, so a
//    new notice shows up as a changed snapshot.
//
// Vendor ids: the sd3 route takes a `model` field (sd3.5-*), so its ids are
// that field's values; core and ultra have no model field — the route selects
// the model — so their ids are the route slugs; SDXL is the v1 engine id.
// Mapping mirrors litegen-core/src/providers/image/stability.rs `resolve_model`
// and `v2_endpoint`.
import { openapiModelIds, openapiExcerpt, markdownExcerpt, parseYaml, deref } from '../extract.mjs';

const GENERATE_ROUTE = /^\/v2beta\/stable-image\/generate\/([^/]+)$/;
const V2_ENDPOINTS = [
  '/v2beta/stable-image/generate/sd3',
  '/v2beta/stable-image/generate/core',
  '/v2beta/stable-image/generate/ultra',
  '/v2beta/stable-image/edit/inpaint',
  '/v2beta/stable-image/edit/remove-background',
  '/v2beta/stable-image/edit/outpaint',
  '/v2beta/stable-image/edit/search-and-replace',
];
// The v1 reference pages: source key → [docs.stability.ai/api-reference page,
// method, path]. The adapter calls the three generation operations; the
// engines list is where Stability names the live engine ids.
const V1_PAGES = {
  'v1-text-to-image': ['sdxl-10/text-to-image', 'post', '/v1/generation/{engine_id}/text-to-image'],
  'v1-image-to-image': ['sdxl-10/image-to-image-with-prompt', 'post', '/v1/generation/{engine_id}/image-to-image'],
  'v1-masking': ['sdxl-10/image-to-image-with-a-mask', 'post', '/v1/generation/{engine_id}/image-to-image/masking'],
  'v1-engines': ['engines/list-engines', 'get', '/v1/engines/list'],
};

// The sd3 `model` enum lags the models the same field's description prices:
// sd3.5-flash is billed on this route but missing from the enum. Backticked ids
// in that description count too; the retired sd3-* names it lists are
// acknowledged below.
function describedModels(spec, path) {
  const local = (node) => (node?.$ref?.startsWith('#/components/schemas/') ? spec.components.schemas[node.$ref.split('/').pop()] : node);
  const schema = local(spec.paths[path].post.requestBody?.content?.['multipart/form-data']?.schema);
  const text = local(schema?.properties?.model)?.description ?? '';
  return [...text.matchAll(/`([a-z0-9][\w.-]*)`/g)].map((m) => m[1]);
}

function generateModels(spec) {
  const ids = [];
  for (const [path, ops] of Object.entries(spec.paths)) {
    const route = path.match(GENERATE_ROUTE)?.[1];
    if (!route || !ops.post) continue;
    const enumValues = openapiModelIds(spec, { paths: [path] });
    ids.push(...(enumValues.length ? [...enumValues, ...describedModels(spec, path)] : [route]));
  }
  // The carried edit routes (the adapter calls no others).
  for (const route of ['edit/inpaint', 'edit/remove-background', 'edit/outpaint', 'edit/search-and-replace']) {
    if (spec.paths[`/v2beta/stable-image/${route}`]?.post) ids.push(route);
  }
  return ids;
}

// Engine ids the v1 operations document (descriptions, code samples, and the
// parameters, request bodies and responses they reference, resolved: the
// engines list names its engines in its response example). Not orphaned
// components: the full spec carried an unreferenced `upscaleEngineID` example
// (esrgan-v1-x2plus, retired 2024-10-11 with its endpoint).
function v1Engines(spec) {
  const text = Object.values(spec.paths)
    .flatMap((ops) => Object.values(ops))
    .map((op) => JSON.stringify(deref(spec, op)))
    .join('\n');
  return text.match(/\b(?:stable-diffusion|stable-inpainting|esrgan)-[a-z0-9][\w.-]*/g) ?? [];
}

/** The one-path v1 spec a reference page's markdown twin embeds; throws unless it holds `method path`. */
function v1Spec(md, method, path) {
  const spec = parseYaml(markdownExcerpt(md, { lang: 'yaml' }));
  if (!spec?.paths?.[path]?.[method]) throw new Error(`the page embeds no ${method.toUpperCase()} ${path} operation — it moved or was restructured`);
  return spec;
}

/** A v1 page: ids are the engine ids its operation names; typings are the request schema (a GET has none). */
const v1Page = (key, [page, method, path]) => ({
  key,
  url: `https://docs.stability.ai/api-reference/${page}.md`,
  expect: 'text',
  extract: (md) => v1Engines(v1Spec(md, method, path)),
  snapshot: (md, _body, { ours }) => {
    const spec = v1Spec(md, method, path);
    return (
      `# v1 engine_id values the page names\n${[...new Set(v1Engines(spec))].sort().join('\n')}\n\n` +
      (method === 'get' ? `# ${method.toUpperCase()} ${path}\n` : openapiExcerpt(spec, { paths: [path], ours, method }))
    );
  },
});

// release-notes.md: each entry opens with a bold date on its own line
// (`**April 17, 2025**`) and a one-line title after it.
const DATE_LINE = /^\*\*([A-Z][a-z]{2,8}\.? \d{1,2}(?:st|nd|rd|th)?,? \d{4})\*\*\s*$/;
function releaseNotes(md) {
  const lines = md.split('\n');
  const notes = [];
  lines.forEach((line, i) => {
    const date = line.match(DATE_LINE)?.[1];
    if (!date) return;
    const title = lines.slice(i + 1).find((l) => l.trim())?.trim().replace(/^#+\s*/, '');
    notes.push(`${date} · ${title ?? '(untitled)'}`);
  });
  if (notes.length === 0) throw new Error('release-notes.md has no `**date**` entries — restructured');
  return notes;
}

export default {
  provider: 'stability',
  models: {
    'stability/sd3-large': 'sd3.5-large',
    'stability/sd3-turbo': 'sd3.5-large-turbo',
    'stability/sd3.5-medium': 'sd3.5-medium',
    'stability/core': 'core',
    'stability/ultra': 'ultra',
    'stability/sdxl': 'stable-diffusion-xl-1024-v1-0',
    'stability/inpaint': 'edit/inpaint',
    'stability/remove-background': 'edit/remove-background',
    'stability/outpaint': 'edit/outpaint',
    'stability/search-and-replace': 'edit/search-and-replace',
  },
  sources: [
    {
      key: 'openapi',
      url: 'https://api.stability.ai/v2alpha/openapi',
      expect: 'json',
      extract: generateModels,
      snapshot: (spec, _body, { ours }) =>
        `# stable-image endpoints\n${Object.keys(spec.paths).filter((p) => p.startsWith('/v2beta/stable-image/') && spec.paths[p].post).sort().join('\n')}\n\n` +
        openapiExcerpt(spec, { paths: V2_ENDPOINTS, ours }),
    },
    ...Object.entries(V1_PAGES).map(([key, page]) => v1Page(key, page)),
    {
      key: 'release-notes',
      index: true,
      url: 'https://docs.stability.ai/release-notes.md',
      expect: 'text',
      extract: (md) => releaseNotes(md),
      snapshot: (md) => releaseNotes(md).join('\n') + '\n',
    },
  ],
  acknowledged: [
    ...['sd3-large', 'sd3-large-turbo', 'sd3-medium'].map((id) => ({
      id,
      reason: 'SD3.0 API deprecated 2025-04-17; calls are re-routed to the sd3.5-* equivalent (the sd3 `model` description and the platform changelog say so)',
    })),
    {
      id: 'sd3.5-flash',
      reason: 'skipped 2026-09-11: the sd3 `model` description prices it, but the `model` enum does not list it — revisit when the enum does',
    },
  ],
};
