// fal.ai — an aggregator (~1,500 endpoints), so "new" is scoped to the model
// families we carry: FLUX, Stable Diffusion / SDXL, Recraft, AuraFlow and LTX
// Video, matched on the endpoint id, minus training endpoints (LoRA trainers
// are not generation models). Sources are fal's public Platform API
// (api.fal.ai/v1/models — no key needed, but unauthenticated calls are
// rate-limited, so this stays at three fetches):
//  - catalog-1/-2: list mode, which returns every *active* endpoint (deprecated
//    ones drop out) at most 1,000 per page. Two pages cover the catalog today;
//    catalog-2 fails loudly once a third page exists. List order follows
//    popularity, so which page an id lands on churns week to week — these two
//    snapshots are a fixed note, not the id list, to keep "changed" meaningful.
//  - carried: find mode for exactly our endpoint ids with expand=openapi-3.0 —
//    their registry status plus each endpoint's input schema (the typings
//    snapshot). An id the registry no longer knows comes back without metadata
//    and counts as absent.
// Vendor id = the endpoint id in the URL path. Mapping mirrors
// litegen-core/src/providers/image/fal.rs `resolve_endpoint` and
// video/fal.rs `resolve_spec` (fal/video is its LTX Video default arm).
import { openapiExcerpt } from '../extract.mjs';

const MODELS = {
  'fal/flux-pro': 'fal-ai/flux-pro/v1.1',
  'fal/flux-dev': 'fal-ai/flux/dev',
  'fal/flux-schnell': 'fal-ai/flux/schnell',
  'fal/sdxl': 'fal-ai/fast-sdxl',
  'fal/sd35-medium': 'fal-ai/stable-diffusion-v35-medium',
  'fal/recraft-v3': 'fal-ai/recraft-v3',
  'fal/auraflow': 'fal-ai/aura-flow',
  'fal/video': ['fal-ai/ltx-video', 'fal-ai/ltx-video/image-to-video'],
};

const API = 'https://api.fal.ai/v1/models';
const FAMILIES = /(?:^|[/-])(?:flux|stable-diffusion|sdxl|sd3|recraft|aura-?flow|ltx)/;

const isTraining = (m) => m.metadata?.category === 'training' || m.metadata?.kind === 'training';

function catalogIds(page, { last }) {
  if (!Array.isArray(page.models)) throw new Error('response has no `models` array');
  if (page.models.length === 0) throw new Error('empty catalog page — the catalog shrank below this page; drop the source');
  if (last && page.has_more) throw new Error('the catalog outgrew the pages this definition fetches — add a source with the next cursor');
  return page.models.filter((m) => FAMILIES.test(m.endpoint_id) && !isTraining(m)).map((m) => m.endpoint_id);
}

const CATALOG_NOTE =
  "# ids from this page are compared as a union with the other catalog page; the page split follows fal's popularity order, so it is not snapshotted\n";

const carriedIds = Object.values(MODELS).flat();

// Registry status and input schema of each carried endpoint, sorted by id.
function carriedSnapshot(res, ours) {
  return [...res.models]
    .sort((a, b) => a.endpoint_id.localeCompare(b.endpoint_id))
    .map((m) => {
      const status = m.metadata ? `${m.metadata.status} ${m.metadata.category}` : 'not in the model registry';
      const schema = m.openapi ? openapiExcerpt(m.openapi, { paths: [`/${m.endpoint_id}`], ours }) : '(no openapi)\n';
      return `## ${m.endpoint_id} — ${status}\n${schema}`;
    })
    .join('\n');
}

export default {
  provider: 'fal',
  models: MODELS,
  sources: [
    {
      key: 'catalog-1',
      url: `${API}?limit=1000`,
      expect: 'json',
      extract: (page) => catalogIds(page, { last: false }),
      snapshot: () => CATALOG_NOTE,
    },
    {
      key: 'catalog-2',
      url: `${API}?limit=1000&cursor=Mg==`,
      expect: 'json',
      extract: (page) => catalogIds(page, { last: true }),
      snapshot: () => CATALOG_NOTE,
    },
    {
      key: 'carried',
      url: `${API}?${carriedIds.map((id) => `endpoint_id=${encodeURIComponent(id)}`).join('&')}&expand=openapi-3.0`,
      expect: 'json',
      extract: (res) => {
        if (!Array.isArray(res.models)) throw new Error('response has no `models` array');
        return res.models.filter((m) => m.metadata?.status === 'active').map((m) => m.endpoint_id);
      },
      snapshot: (res, _body, { ours }) => carriedSnapshot(res, ours),
    },
  ],
  acknowledged: [
    { pattern: /\/stream$/, reason: 'streaming transport of the parent endpoint (same model and input schema), not a separate model' },
  ],
};
