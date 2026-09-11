// Stability AI — the two OpenAPI specs the platform docs (a JS app) render:
//  - openapi: https://api.stability.ai/v2alpha/openapi — the docs app's
//    VITE_REST_API_SPEC_URL. Despite the path it is the "v2beta" spec with the
//    Stable Image generate routes (sd3 / core / ultra).
//  - docs-bundle: the "Version 1" spec (SDXL 1.0, /v1/generation/{engine}/…) is
//    no longer served anywhere (the docs' VITE_LEGACY_REST_API_SPEC_URL,
//    api.stability.ai/openapi.json, is 404); the docs app embeds it in its JS
//    bundle and merges it in client-side. The bundle name is content-hashed and
//    changes on every docs deploy, so the source `discover`s it from the
//    `/assets/index-*.js` <script src> of the docs page; it fails loudly if the
//    page stops linking a bundle like that.
//
// Vendor ids: the sd3 route takes a `model` field (sd3.5-*), so its ids are
// that field's values; core and ultra have no model field — the route selects
// the model — so their ids are the route slugs; SDXL is the v1 engine id.
// Mapping mirrors litegen-core/src/providers/image/stability.rs `resolve_model`
// and `v2_endpoint`.
import { openapiModelIds, openapiExcerpt } from '../extract.mjs';

const DOCS_BUNDLE = { url: 'https://platform.stability.ai/docs/api-reference', pattern: /\/assets\/index-[\w-]+\.js/ };

const GENERATE_ROUTE = /^\/v2beta\/stable-image\/generate\/([^/]+)$/;
const V2_ENDPOINTS = ['/v2beta/stable-image/generate/sd3', '/v2beta/stable-image/generate/core', '/v2beta/stable-image/generate/ultra'];
const V1_ENDPOINTS = [
  '/v1/generation/{engine_id}/text-to-image',
  '/v1/generation/{engine_id}/image-to-image',
  '/v1/generation/{engine_id}/image-to-image/masking',
];

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
  return ids;
}

// A JS string literal's content, from just after its opening quote to the
// first unescaped closing quote, with the JS-level escapes undone (the JSON
// escapes inside stay, so the result is the JSON text the bundle parses).
function jsLiteral(body, from, quote) {
  let i = from;
  while (i < body.length && body[i] !== quote) i += body[i] === '\\' ? 2 : 1;
  if (i >= body.length) throw new Error('unterminated string literal in the docs bundle');
  return body.slice(from, i).replace(/\\([\\`$'"])/g, '$1');
}

// The embedded Version 1 spec: `paths=JSON.parse(`{"/v1/generation/…`)` and
// `components=JSON.parse('{"schemas":…')`, as esbuild inlines a JSON import.
function v1Spec(body) {
  const paths = body.match(/JSON\.parse\(([`'])(?=\{"\/v1\/generation\/)/);
  if (!paths) throw new Error('the Version 1 (SDXL) OpenAPI spec is no longer embedded in the docs bundle');
  const from = paths.index + paths[0].length;
  const components = body.slice(from).match(/JSON\.parse\(([`'])(?=\{"schemas":)/);
  if (!components) throw new Error('the Version 1 spec components are no longer embedded in the docs bundle');
  const cFrom = from + components.index + components[0].length;
  return {
    paths: JSON.parse(jsLiteral(body, from, paths[1])),
    components: JSON.parse(jsLiteral(body, cFrom, components[1])),
  };
}

// Engine ids the v1 operations document (descriptions, code samples, and the
// path parameters they reference). Not the whole bundle — its release notes
// name long-retired engines — and not orphaned components: the spec still
// carries an unreferenced `upscaleEngineID` example (esrgan-v1-x2plus, retired
// 2024-10-11 with its endpoint).
function v1Engines(body) {
  const spec = v1Spec(body);
  const param = (p) => (p.$ref?.startsWith('#/components/parameters/') ? spec.components.parameters?.[p.$ref.split('/').pop()] : p);
  const text = Object.values(spec.paths)
    .flatMap((ops) => Object.values(ops))
    .map((op) => JSON.stringify(op) + JSON.stringify((op.parameters ?? []).map(param)))
    .join('\n');
  return text.match(/\b(?:stable-diffusion|stable-inpainting|esrgan)-[a-z0-9][\w.-]*/g) ?? [];
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
    {
      key: 'docs-bundle',
      discover: DOCS_BUNDLE,
      expect: 'text',
      extract: (_text, body) => v1Engines(body),
      snapshot: (_text, body, { ours }) => {
        const spec = v1Spec(body);
        return `# v1 endpoints\n${Object.keys(spec.paths).sort().join('\n')}\n\n` + openapiExcerpt(spec, { paths: V1_ENDPOINTS, ours });
      },
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
