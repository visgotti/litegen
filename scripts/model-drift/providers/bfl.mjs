// Black Forest Labs — the OpenAPI spec the API itself serves (FastAPI). Every
// model is its own endpoint, `POST /v1/{model}`, so the vendor id is the path
// after /v1/ (the body has no `model` field). The docs (docs.bfl.ml, llms.txt)
// render their API reference from this same spec, so they add no second signal.
// Mapping mirrors litegen-core/src/providers/image/bfl.rs `resolve_model`
// (strip `bfl/`; the adapter POSTs to https://api.bfl.ai/v1/{id}).
import { openapiExcerpt } from '../extract.mjs';

const MODELS = {
  'bfl/flux-pro-1.1': 'flux-pro-1.1',
  'bfl/flux-dev': 'flux-dev',
  'bfl/flux-pro-1.1-ultra': 'flux-pro-1.1-ultra',
  'bfl/flux-kontext-pro': 'flux-kontext-pro',
  'bfl/flux-kontext-max': 'flux-kontext-max',
  'bfl/flux-2-pro': 'flux-2-pro',
  'bfl/flux-2-max': 'flux-2-max',
  'bfl/flux-2-klein-9b': 'flux-2-klein-9b',
};

// The endpoints the adapter calls — one per carried model.
const ENDPOINTS = Object.values(MODELS).map((id) => `/v1/${id}`);

// Model endpoints: POSTs under /v1/ that the spec tags "Models", or that are
// named flux-* so a retag cannot silently drop one. This includes the FLUX
// Tools (/v1/flux-tools/erase-v1, …): they are model endpoints BFL sells, and
// the id is exactly the path segment the adapter would send. Utility POSTs
// (/v1/delete_finetune) are neither.
const modelEndpoints = (spec) =>
  Object.entries(spec.paths)
    .filter(([path, ops]) => path.startsWith('/v1/') && ops.post && ((ops.post.tags ?? []).includes('Models') || path.startsWith('/v1/flux-')))
    .map(([path]) => path.slice('/v1/'.length));

export default {
  provider: 'bfl',
  models: MODELS,
  sources: [
    {
      key: 'openapi',
      url: 'https://api.bfl.ai/openapi.json',
      expect: 'json',
      extract: modelEndpoints,
      snapshot: (spec, _body, { ours }) => openapiExcerpt(spec, { paths: ENDPOINTS, ours }),
    },
  ],
  // Preview channels are listed by id, not by a -preview pattern, so a model
  // that launches preview-only still surfaces as new.
  acknowledged: [
    ...['flux-2-pro-preview', 'flux-2-klein-9b-preview'].map((id) => ({
      id,
      reason: `preview channel of ${id.replace(/-preview$/, '')} — docs: "where our latest … improvements land first. For stable production use, prefer" the un-suffixed endpoint`,
    })),
    {
      id: 'flux-pro-1.1-ultra-finetuned',
      reason: 'FLUX.1 Finetuning API deprecated 2025-10-31 (docs.bfl.ml/release-notes: "No migration path available"); the spec still lists it',
    },
    {
      id: 'flux-pro-1.0-fill-finetuned',
      reason: 'FLUX.1 Finetuning API deprecated 2025-10-31 (docs.bfl.ml/release-notes: "No migration path available"); the spec still lists it',
    },
    { id: 'flux-2-klein-4b', reason: 'skipped 2026-09-11: a near-duplicate of flux-2-klein-9b, which we carry' },
    { id: 'flux-2-flex', reason: "skipped 2026-09-11: BFL's own price pages conflict ($0.05 vs $0.06 per MP)" },
    { id: 'flux-3-video', reason: 'skipped 2026-09-11: FLUX 3 is video; it needs a BFL video adapter' },
    ...['flux-pro-1.0-fill', 'flux-pro-1.0-expand'].map((id) => ({
      id,
      reason: 'skipped 2026-09-11: FLUX.1 Fill / Expand are edit tools that need image (and mask) inputs',
    })),
    { pattern: /^flux-tools\//, reason: 'skipped 2026-09-11: FLUX Tools (erase, deblur, outpainting, try-on, video edit/upscale) are edit tools, not generation models' },
  ],
};
