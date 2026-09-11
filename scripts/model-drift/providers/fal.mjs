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
  'fal/flux-2': 'fal-ai/flux-2',
  'fal/flux-schnell': 'fal-ai/flux/schnell',
  'fal/sdxl': 'fal-ai/fast-sdxl',
  'fal/sd35-medium': 'fal-ai/stable-diffusion-v35-medium',
  // Renamed from fal-ai/recraft-v3 (no longer in the model registry; its page 308-redirects here).
  'fal/recraft-v3': 'fal-ai/recraft/v3/text-to-image',
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

// Carried endpoints whose Platform API spec is written for an alias id. For
// fal-ai/flux-2 the registry entry is active with model_url
// https://fal.run/fal-ai/flux-2 (and fal.ai's per-endpoint queue spec uses
// /fal-ai/flux-2), but the expand=openapi-3.0 spec it returns is titled for
// fal-ai/flux-2-dev, with paths under /fal-ai/flux-2-dev (seen 2026-09-11). The
// alias is listed explicitly so any other spec/path mismatch still fails.
const SPEC_ALIAS = { 'fal-ai/flux-2': 'fal-ai/flux-2-dev' };

// Registry status and input schema of each carried endpoint, sorted by id.
function carriedSnapshot(res, ours) {
  return [...res.models]
    .sort((a, b) => a.endpoint_id.localeCompare(b.endpoint_id))
    .map((m) => {
      const status = m.metadata ? `${m.metadata.status} ${m.metadata.category}` : 'not in the model registry';
      const specId = SPEC_ALIAS[m.endpoint_id] ?? m.endpoint_id;
      const schema = m.openapi ? openapiExcerpt(m.openapi, { paths: [`/${specId}`], ours }) : '(no openapi)\n';
      const alias = specId === m.endpoint_id ? '' : ` (spec written for ${specId})`;
      return `## ${m.endpoint_id} — ${status}${alias}\n${schema}`;
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
    // Skips decided 2026-09-11 (docs/superpowers/2026-09-11-model-drift-decisions.md).
    // Deliberately NOT acknowledged, so the weekly report keeps showing them:
    // fal-ai/ltx-2.3/{text,image}-to-video and fal-ai/ltx-video-13b-distilled
    // (+ its /image-to-video), the candidate successors for fal/video's LTX
    // endpoints ("add later").
    {
      pattern: /^blackforestlabs\/flux-3\//,
      reason: "skipped 2026-09-11: FLUX 3 video — decide together with BFL's own flux-3-video",
    },
    {
      id: 'blackforestlabs/flux-video-upscale',
      reason: "skipped 2026-09-11: a FLUX video tool, skipped with BFL's own flux-tools/*",
    },
    {
      pattern: /^fal-ai\/flux-2-(?:pro|max|flex)(?:\/|$)|^fal-ai\/flux-2\/klein\//,
      reason: 'skipped 2026-09-11: FLUX.2 [pro]/[max]/[flex]/[klein] re-hosts — BFL direct is our FLUX.2 route (bfl/flux-2-pro, -max, -klein-9b; flex and klein 4B are skipped there too)',
    },
    {
      pattern: /^fal-ai\/flux-2\/(?:edit|lora|turbo|flash)(?:\/|$)|^fal-ai\/flux-2-lora-gallery\//,
      reason: 'skipped 2026-09-11: FLUX.2 [dev] edit, LoRA, turbo/flash and LoRA-gallery variants; we carry the base fal-ai/flux-2',
    },
    {
      pattern:
        /^fal-ai\/flux-1\/|^fal-ai\/flux\/(?:dev|schnell)\/|^fal-ai\/flux\/(?:krea|srpo)(?:\/|$)|^fal-ai\/flux-(?:general|kontext|kontext-lora|krea-lora|lora|lora-canny|lora-depth|lora-fill)(?:\/|$)|^fal-ai\/flux-control-lora-(?:canny|depth)(?:\/|$)|^rundiffusion-fal\/(?:juggernaut-flux|rundiffusion-photo-flux)/,
      reason: 'skipped 2026-09-11: FLUX.1 re-hosts, LoRA, control and community variants',
    },
    ...['fal-ai/flux-pulid', 'fal-ai/flux-subject', 'fal-ai/flux-vision-upscaler'].map((id) => ({
      id,
      reason: 'skipped 2026-09-11: FLUX.1 re-hosts, LoRA, control and community variants',
    })),
    {
      pattern: /^fal-ai\/flux-pro\/(?:kontext|v1\.1-ultra|v1\.1\/redux|v1\/)/,
      reason: 'skipped 2026-09-11: FLUX.1 [pro] variants — BFL direct covers them',
    },
    {
      pattern:
        /^fal-ai\/(?:fast-fooocus-sdxl|fast-lightning-sdxl|fast-sdxl-controlnet-canny|sdxl-controlnet-union)(?:\/|$)|^fal-ai\/fast-sdxl\/|^fal-ai\/stable-diffusion-v(?:15|3-medium|35-large)(?:\/|$)/,
      reason: 'skipped 2026-09-11: SDXL and SD 1.5/3/3.5 variants',
    },
    {
      pattern: /^fal-ai\/recraft(?:-20b$|\/(?:upscale|v4|v4\.1)\/|\/vectorize$|\/v3\/image-to-image$)|^recraft\/v4\/style\//,
      reason: 'skipped 2026-09-11: Recraft — Recraft direct covers it',
    },
    {
      pattern:
        /^fal-ai\/ltx-2-19b\/|^fal-ai\/ltx-2\.3-(?:22b|quality)\/|^fal-ai\/ltx-2\.3\/(?:audio-to-video|extend-video|reframe|retake-video|(?:text|image)-to-video\/fast)$|^fal-ai\/ltx-video-13b-distilled\/(?:extend|multiconditioning)$|^fal-ai\/ltx-video-v095(?:\/|$)|^fal-ai\/ltxv-13b-098-distilled(?:\/|$)|^lightricks\/ltx-2\.5\//,
      reason: 'skipped 2026-09-11: the other LTX endpoints (fal/video successors are chosen among ltx-2.3 and ltx-video-13b-distilled)',
    },
  ],
};
