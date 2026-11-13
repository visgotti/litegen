// fal.ai — an aggregator (~1,500 endpoints), so "new" is scoped to the model
// families we carry: FLUX, Stable Diffusion / SDXL, Recraft, AuraFlow, SAM
// (segmentation), Bria and BiRefNet (background removal), LTX Video, and the 3D families Hyper3D Rodin, TRELLIS,
// Hunyuan3D, Meshy and Tripo, matched on
// the endpoint id, minus training endpoints (LoRA trainers are not generation
// models). Sources are fal's public Platform API (api.fal.ai/v1/models — no
// key needed, but unauthenticated calls are rate-limited, so this stays at five
// fetches):
//  - catalog-1/-2: list mode, which returns every *active* endpoint (deprecated
//    ones drop out) at most 1,000 per page. Two pages cover the catalog today;
//    catalog-2 fails loudly once a third page exists. List order follows
//    popularity, so which page an id lands on churns week to week — these two
//    snapshots are a fixed note, not the id list, to keep "changed" meaningful.
//  - carried-image/-video/-3d: find mode for exactly our endpoint ids with
//    expand=openapi-3.0 — their registry status plus each endpoint's input
//    schema (the typings snapshot). An id the registry no longer knows comes
//    back without metadata and counts as absent. Find mode also returns an
//    endpoint fal marks deprecated (status "deprecated": still served, the
//    list mode above drops it), and `deprecated` reports it as such rather
//    than gone; a shutdown date or replacement fal sets on it (`retires_at`,
//    `replacement`) shows in its heading, so it reads as a changed snapshot.
//    That spec is the same document
//    the adapters cite as https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=…
//    (byte-identical for the three 3D endpoints, 2026-10-12), so per-endpoint
//    fetches of it would add calls without adding typings.
// Vendor id = the endpoint id in the URL path. Mapping mirrors
// litegen-core/src/providers/image/fal.rs `resolve_endpoint` and
// `SEGMENT_MODELS`, video/fal.rs
// `resolve_spec` (fal/video is its LTX Video default arm; fal/ltx-2.3 has its
// own arm) and model3d/fal.rs `resolve_spec` (text_to_3d, image_to_3d and
// multiview_to_3d).
import { openapiExcerpt } from '../extract.mjs';

// Grouped by the adapter that calls them; each group is its own typings
// snapshot (carried-<group>), so adding a model to one medium never moves
// another medium's schemas between snapshot files.
const IMAGE = {
  'fal/flux-pro': 'fal-ai/flux-pro/v1.1',
  // FLUX.1 [pro] variants (2026-11-04). Kontext: text-to-image, one image, several.
  'fal/flux-kontext-pro': ['fal-ai/flux-pro/kontext/text-to-image', 'fal-ai/flux-pro/kontext', 'fal-ai/flux-pro/kontext/multi'],
  'fal/flux-kontext-max': ['fal-ai/flux-pro/kontext/max/text-to-image', 'fal-ai/flux-pro/kontext/max', 'fal-ai/flux-pro/kontext/max/multi'],
  'fal/flux-pro-1.1-ultra': 'fal-ai/flux-pro/v1.1-ultra',
  'fal/flux-pro-1.1-ultra-redux': 'fal-ai/flux-pro/v1.1-ultra/redux',
  'fal/flux-pro-1.1-redux': 'fal-ai/flux-pro/v1.1/redux',
  'fal/flux-pro-fill': 'fal-ai/flux-pro/v1/fill',
  'fal/flux-pro-erase': 'fal-ai/flux-pro/v1/erase',
  'fal/flux-dev': 'fal-ai/flux/dev',
  // FLUX.2 (2026-11-04): each model's text-to-image endpoint and the /edit twin the
  // adapter calls with images.
  'fal/flux-2': ['fal-ai/flux-2', 'fal-ai/flux-2/edit'],
  'fal/flux-2-turbo': ['fal-ai/flux-2/turbo', 'fal-ai/flux-2/turbo/edit'],
  'fal/flux-2-flash': ['fal-ai/flux-2/flash', 'fal-ai/flux-2/flash/edit'],
  'fal/flux-2-pro': ['fal-ai/flux-2-pro', 'fal-ai/flux-2-pro/edit'],
  'fal/flux-2-max': ['fal-ai/flux-2-max', 'fal-ai/flux-2-max/edit'],
  'fal/flux-2-flex': ['fal-ai/flux-2-flex', 'fal-ai/flux-2-flex/edit'],
  'fal/flux-2-pro-outpaint': 'fal-ai/flux-2-pro/outpaint',
  'fal/flux-2-klein-4b': ['fal-ai/flux-2/klein/4b', 'fal-ai/flux-2/klein/4b/edit'],
  'fal/flux-2-klein-9b': ['fal-ai/flux-2/klein/9b', 'fal-ai/flux-2/klein/9b/edit'],
  'fal/flux-2-klein-4b-base': ['fal-ai/flux-2/klein/4b/base', 'fal-ai/flux-2/klein/4b/base/edit'],
  'fal/flux-2-klein-9b-base': ['fal-ai/flux-2/klein/9b/base', 'fal-ai/flux-2/klein/9b/base/edit'],
  // FLUX 3 as fal routes (2026-11-04): text-to-image, and the edit twin the adapter calls with images.
  'fal/flux-3-image': ['blackforestlabs/flux-3/text-to-image', 'blackforestlabs/flux-3/edit-image'],
  'fal/flux-3-image-klein': ['blackforestlabs/flux-3/klein/text-to-image', 'blackforestlabs/flux-3/klein/edit-image'],
  // FLUX 3 Klein base (undistilled), which fal serves only at /lora endpoints; the adapter sends no `loras` (2026-11-04).
  'fal/flux-3-image-klein-base': ['blackforestlabs/flux-3/klein/base/text-to-image/lora', 'blackforestlabs/flux-3/klein/base/edit-image/lora'],
  'fal/flux-schnell': 'fal-ai/flux/schnell',
  'fal/sdxl': 'fal-ai/fast-sdxl',
  'fal/sd35-medium': 'fal-ai/stable-diffusion-v35-medium',
  // Renamed from fal-ai/recraft-v3 (no longer in the model registry; its page 308-redirects here).
  'fal/recraft-v3': ['fal-ai/recraft/v3/text-to-image', 'fal-ai/recraft/v3/image-to-image'],
  // Recraft V2 (the listed fal-ai/recraft-20b) and V4 / V4.1 (2026-11-04).
  'fal/recraft-v2': 'fal-ai/recraft-20b',
  'fal/recraft-v4': 'fal-ai/recraft/v4/text-to-image',
  'fal/recraft-v4-pro': 'fal-ai/recraft/v4/pro/text-to-image',
  'fal/recraft-v4-vector': 'fal-ai/recraft/v4/text-to-vector',
  'fal/recraft-v4-pro-vector': 'fal-ai/recraft/v4/pro/text-to-vector',
  'fal/recraft-v4.1': 'fal-ai/recraft/v4.1/text-to-image',
  'fal/recraft-v4.1-pro': 'fal-ai/recraft/v4.1/pro/text-to-image',
  'fal/recraft-v4.1-vector': 'fal-ai/recraft/v4.1/text-to-vector',
  'fal/recraft-v4.1-pro-vector': 'fal-ai/recraft/v4.1/pro/text-to-vector',
  'fal/recraft-v4.1-utility': 'fal-ai/recraft/v4.1/utility/text-to-image',
  'fal/recraft-v4.1-utility-pro': 'fal-ai/recraft/v4.1/utility/pro/text-to-image',
  'fal/recraft-v4.1-flash': 'recraft/v4.1/flash/text-to-image',
  'fal/recraft-v4-style': 'recraft/v4/style/text-to-image',
  'fal/recraft-v4-style-pro': 'recraft/v4/style/pro/text-to-image',
  'fal/recraft-v4-style-vector': 'recraft/v4/style/text-to-vector',
  'fal/recraft-v4-style-pro-vector': 'recraft/v4/style/pro/text-to-vector',
  'fal/recraft-vectorize': 'fal-ai/recraft/vectorize',
  'fal/recraft-upscale-crisp': 'fal-ai/recraft/upscale/crisp',
  'fal/recraft-upscale-creative': 'fal-ai/recraft/upscale/creative',
  'fal/auraflow': 'fal-ai/aura-flow',
  // Segmentation (image/fal.rs `SEGMENT_MODELS`), appended so the chunks above keep their ids.
  'fal/sam-3': 'fal-ai/sam-3/image',
  'fal/sam-3.1': 'fal-ai/sam-3-1/image',
  // Background removal (image/fal.rs `resolve_endpoint`).
  'fal/bria-rmbg-2': 'fal-ai/bria/background/remove',
  'fal/birefnet-v2': 'fal-ai/birefnet/v2',
};
const VIDEO = {
  'fal/video': ['fal-ai/ltx-video', 'fal-ai/ltx-video/image-to-video'],
  'fal/ltx-2.3': ['fal-ai/ltx-2.3/text-to-video', 'fal-ai/ltx-2.3/image-to-video'],
  // FLUX 3 video (2026-11-04): each row reaches text-to-video, image-to-video, the
  // first-last frame endpoint (Max: image-to-video's end_image_url) and keyframes.
  'fal/flux-3-video': ['blackforestlabs/flux-3/text-to-video', 'blackforestlabs/flux-3/image-to-video', 'blackforestlabs/flux-3/first-last-frame-to-video', 'blackforestlabs/flux-3/keyframes-to-video'],
  'fal/flux-3-video-draft': ['blackforestlabs/flux-3/text-to-video/draft', 'blackforestlabs/flux-3/image-to-video/draft', 'blackforestlabs/flux-3/first-last-frame-to-video/draft', 'blackforestlabs/flux-3/keyframes-to-video/draft'],
  'fal/flux-3-max-video': ['blackforestlabs/flux-3-max/text-to-video', 'blackforestlabs/flux-3-max/image-to-video', 'blackforestlabs/flux-3-max/keyframes-to-video'],
};
const MODEL3D = {
  // One endpoint for both modes: text_to_3d and image_to_3d are both fal-ai/hyper3d/rodin.
  'fal/hyper3d-rodin': 'fal-ai/hyper3d/rodin',
  // Image-to-3D only (text_to_3d is empty): image_url / input_image_url is required.
  'fal/trellis': 'fal-ai/trellis',
  'fal/hunyuan3d-v2': 'fal-ai/hunyuan3d/v2',
  // Multiview only (multiview_to_3d): image_urls, in Tripo's view order.
  'fal/tripo-h3.1-multiview': 'tripo3d/h3.1/multiview-to-3d',
  // One endpoint for both image_to_3d and multiview_to_3d: a field per view.
  'fal/hunyuan3d-v3.1-pro': 'fal-ai/hunyuan-3d/v3.1/pro/image-to-3d',
  // An operation on a mesh (model3d/fal.rs `resolve_operation`): model_url in.
  'fal/meshy-rig': 'fal-ai/meshy/rigging',
};
const GROUPS = { image: IMAGE, video: VIDEO, '3d': MODEL3D };
const MODELS = { ...IMAGE, ...VIDEO, ...MODEL3D };

const API = 'https://api.fal.ai/v1/models';
const FAMILIES = /(?:^|[/-])(?:flux|stable-diffusion|sdxl|sd3|recraft|aura-?flow|ltx|hyper3d|rodin|trellis|hunyuan-?3d|meshy|tripo|sam|bria|birefnet)/;

const isTraining = (m) => m.metadata?.category === 'training' || m.metadata?.kind === 'training';
// Every 3D endpoint is in scope whatever its family is called: fal adds 3D
// vendors faster than a name list can follow (fal audit F31, 2026-10-30).
const THREE_D_CATEGORIES = new Set(['image-to-3d', 'text-to-3d', '3d-to-3d']);
const inScope = (m) => FAMILIES.test(m.endpoint_id) || THREE_D_CATEGORIES.has(m.metadata?.category);

function catalogIds(page, { last }) {
  if (!Array.isArray(page.models)) throw new Error('response has no `models` array');
  if (page.models.length === 0) throw new Error('empty catalog page — the catalog shrank below this page; drop the source');
  if (last && page.has_more) throw new Error('the catalog outgrew the pages this definition fetches — add a source with the next cursor');
  return page.models.filter((m) => inScope(m) && !isTraining(m)).map((m) => m.endpoint_id);
}

const CATALOG_NOTE =
  "# ids from this page are compared as a union with the other catalog page; the page split follows fal's popularity order, so it is not snapshotted\n";

// At most 8 ids per find-mode call (the API caps at 10 and drops the excess;
// an explicit limit above 10 with expand=openapi-3.0 is a 400). A group that
// outgrows a chunk continues in carried-<group>-2, -3, ….
const CARRIED_CHUNK = 8;
const carriedChunks = Object.entries(GROUPS).flatMap(([group, models]) => {
  const ids = Object.values(models).flat();
  return Array.from({ length: Math.ceil(ids.length / CARRIED_CHUNK) }, (_, i) => ({
    key: `carried-${group}${i === 0 ? '' : `-${i + 1}`}`,
    ids: ids.slice(i * CARRIED_CHUNK, (i + 1) * CARRIED_CHUNK),
  }));
});

// Carried endpoints whose Platform API spec is written for an alias id. For
// fal-ai/flux-2 the registry entry is active with model_url
// https://fal.run/fal-ai/flux-2 (and fal.ai's per-endpoint queue spec uses
// /fal-ai/flux-2), but the expand=openapi-3.0 spec it returns is titled for
// fal-ai/flux-2-dev, with paths under /fal-ai/flux-2-dev (seen 2026-09-11). The
// alias is listed explicitly so any other spec/path mismatch still fails.
const SPEC_ALIAS = {
  'fal-ai/flux-2': 'fal-ai/flux-2-dev',
  // Seen 2026-11-04: the rest of FLUX.2 on fal; input schemas identical to the
  // queue specs the adapter cites.
  'fal-ai/flux-2/edit': 'fal-ai/flux-2-dev/edit',
  'fal-ai/flux-2/turbo/edit': 'fal-ai/flux-2-turbo-flash/turbo/edit',
  'fal-ai/flux-2/flash': 'fal-ai/flux-2-turbo-flash/flash',
  'fal-ai/flux-2/flash/edit': 'fal-ai/flux-2-turbo-flash/flash/edit',
  'fal-ai/flux-2/klein/4b': 'fal-ai/flux-2-klein/4b',
  'fal-ai/flux-2/klein/4b/edit': 'fal-ai/flux-2-klein/4b/edit',
  'fal-ai/flux-2/klein/9b': 'fal-ai/flux-2-klein/9b',
  'fal-ai/flux-2/klein/9b/edit': 'fal-ai/flux-2-klein/9b/edit',
  'fal-ai/flux-2/klein/4b/base': 'fal-ai/flux-2-klein/4b/base',
  'fal-ai/flux-2/klein/4b/base/edit': 'fal-ai/flux-2-klein/4b/base/edit',
  'fal-ai/flux-2/klein/9b/base': 'fal-ai/flux-2-klein/9b/base',
  'fal-ai/flux-2/klein/9b/base/edit': 'fal-ai/flux-2-klein/9b/base/edit',
  // Seen 2026-10-30: the queue specs fal.ai serves per endpoint use the ids
  // themselves; the Platform API's are written for these paths.
  'fal-ai/hunyuan-3d/v3.1/pro/image-to-3d': 'fal-ai/hunyuan-3d-pro-image-to-3d/v3.1/pro/image-to-3d',
  'tripo3d/h3.1/multiview-to-3d': 'fal-ai/tripo3d/h3.1/multiview-to-3d',
  // Seen 2026-11-04: FLUX 3's Platform API specs are written for fal-ai/
  // paths. Their input schemas match the per-endpoint queue specs the adapter
  // cites field for field.
  'blackforestlabs/flux-3/text-to-image': 'fal-ai/flux-3-image',
  'blackforestlabs/flux-3/edit-image': 'fal-ai/flux-3-image/edit',
  'blackforestlabs/flux-3/klein/text-to-image': 'fal-ai/flux-3-image-klein-cpu/text-to-image',
  'blackforestlabs/flux-3/klein/edit-image': 'fal-ai/flux-3-image-klein-cpu/edit-image',
  'blackforestlabs/flux-3/klein/base/text-to-image/lora': 'fal-ai/flux-3-image-klein-cpu/base/text-to-image/lora',
  'blackforestlabs/flux-3/klein/base/edit-image/lora': 'fal-ai/flux-3-image-klein-cpu/base/edit-image/lora',
  // FLUX 3 video and FLUX 3 Max ("alligator"), seen 2026-11-04; input schemas
  // identical to the queue specs the adapter cites.
  'blackforestlabs/flux-3/text-to-video': 'fal-ai/flux-3/text-to-video',
  'blackforestlabs/flux-3/text-to-video/draft': 'fal-ai/flux-3/text-to-video/draft',
  'blackforestlabs/flux-3/image-to-video': 'fal-ai/flux-3/image-to-video',
  'blackforestlabs/flux-3/image-to-video/draft': 'fal-ai/flux-3/image-to-video/draft',
  'blackforestlabs/flux-3/first-last-frame-to-video': 'fal-ai/flux-3/first-last-frame-to-video',
  'blackforestlabs/flux-3/first-last-frame-to-video/draft': 'fal-ai/flux-3/first-last-frame-to-video/draft',
  'blackforestlabs/flux-3/keyframes-to-video': 'fal-ai/flux-3/keyframes-to-video',
  'blackforestlabs/flux-3/keyframes-to-video/draft': 'fal-ai/flux-3/keyframes-to-video/draft',
  'blackforestlabs/flux-3-max/text-to-video': 'fal-ai/alligator/text-to-video',
  'blackforestlabs/flux-3-max/image-to-video': 'fal-ai/alligator/image-to-video',
  'blackforestlabs/flux-3-max/keyframes-to-video': 'fal-ai/alligator/keyframes-to-video',
  // Seen 2026-11-04: Recraft V4.1 Flash's Platform API spec sits under fal-ai/.
  'recraft/v4.1/flash/text-to-image': 'fal-ai/recraft/v4.1/flash/text-to-image',
  'recraft/v4/style/text-to-image': 'fal-ai/recraft/v4/style/text-to-image',
  'recraft/v4/style/pro/text-to-image': 'fal-ai/recraft/v4/style/pro/text-to-image',
  'recraft/v4/style/text-to-vector': 'fal-ai/recraft/v4/style/text-to-vector',
  'recraft/v4/style/pro/text-to-vector': 'fal-ai/recraft/v4/style/pro/text-to-vector',
};

// Registry status and input schema of each carried endpoint, sorted by id.
function carriedSnapshot(res, ours) {
  return [...res.models]
    .sort((a, b) => a.endpoint_id.localeCompare(b.endpoint_id))
    .map((m) => {
      const status = m.metadata ? `${m.metadata.status} ${m.metadata.category}` : 'not in the model registry';
      // Only when fal sets them (null today), so an announced shutdown changes this line.
      const lifecycle = [['retires_at', m.retires_at], ['replacement', m.replacement]]
        .filter(([, v]) => v !== null && v !== undefined)
        .map(([k, v]) => ` · ${k} ${typeof v === 'string' ? v : JSON.stringify(v)}`)
        .join('');
      const specId = SPEC_ALIAS[m.endpoint_id] ?? m.endpoint_id;
      const schema = m.openapi ? openapiExcerpt(m.openapi, { paths: [`/${specId}`], ours }) : '(no openapi)\n';
      const alias = specId === m.endpoint_id ? '' : ` (spec written for ${specId})`;
      return `## ${m.endpoint_id} — ${status}${lifecycle}${alias}\n${schema}`;
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
      // A fixed note, not a schema: the carried-* snapshots are the typings.
      typings: false,
    },
    {
      key: 'catalog-2',
      url: `${API}?limit=1000&cursor=Mg==`,
      expect: 'json',
      extract: (page) => catalogIds(page, { last: true }),
      snapshot: () => CATALOG_NOTE,
      typings: false,
    },
    // find mode, in chunks: fal's Platform API answers at most 10 endpoint_id
    // params per call and silently drops the rest (12 requested returned 10 on
    // 2026-09-11), which would quietly shrink this source's schema coverage as
    // models are added. One source per chunk keeps every carried endpoint's
    // input schema snapshotted; a dropped endpoint then shows up as its section
    // disappearing from the snapshot diff.
    ...carriedChunks.map(({ key, ids }) => ({
      key,
      url: `${API}?${ids.map((id) => `endpoint_id=${encodeURIComponent(id)}`).join('&')}&expand=openapi-3.0`,
      expect: 'json',
      extract: (res) => {
        if (!Array.isArray(res.models)) throw new Error('response has no `models` array');
        return res.models.filter((m) => m.metadata?.status === 'active').map((m) => m.endpoint_id);
      },
      // Still served (fal-ai/aura-flow since 2026-10-27): not active, not gone.
      deprecated: (res) => res.models.filter((m) => m.metadata?.status === 'deprecated').map((m) => m.endpoint_id),
      snapshot: (res, _body, { ours }) => carriedSnapshot(res, ours),
    })),
  ],
  acknowledged: [
    { pattern: /\/stream$/, reason: 'streaming transport of the parent endpoint (same model and input schema), not a separate model' },
    // fal audit F32 (2026-10-30): matches the FLUX family name only.
    { pattern: /^fal-ai\/flux-3-action\/so101$/, reason: 'not a media model: a robot-arm action policy (SO-101) whose id happens to match the FLUX family name' },
    // Seen 2026-11-04: the klein *base* /lora endpoints are a different model
    // (undistilled), carried as fal/flux-3-image-klein-base, so this pattern
    // leaves them out.
    {
      pattern: /^blackforestlabs\/flux-3\/klein\/(?:text-to-image|edit-image)\/lora$/,
      reason: 'the LoRA twins of fal/flux-3-image-klein: the same distilled model plus optional custom LoRAs ("loras … maximum 3"), and litegen takes no LoRA input anywhere (2026-11-04)',
    },
    // Skips decided 2026-09-11 (docs/superpowers/2026-09-11-model-drift-decisions.md).
    {
      pattern: /^fal-ai\/ltx-video-13b-distilled(?:\/image-to-video)?$/,
      reason: 'skipped 2026-09-11: superseded by fal/ltx-2.3; fal/video is kept for existing callers',
    },
    {
      // FLUX 3's generation endpoints are carried since 2026-11-04 (fal/flux-3-video,
      // -video-draft, -max-video). edit-video and extend-video stay `new`, the
      // backlog for litegen's edit and extend tasks on fal.
      id: 'blackforestlabs/flux-3/draft-enhance',
      reason: "re-renders a FLUX 3 draft from the draft's draft_cache file, which litegen's video output does not keep (2026-11-04)",
    },
    {
      id: 'blackforestlabs/flux-video-upscale',
      reason: "skipped 2026-09-11: a FLUX video tool, skipped with BFL's own flux-tools/*",
    },
    // FLUX.2 [pro] (and its outpaint), [max], [flex], [klein] and dev's turbo,
    // flash and edit are carried since 2026-11-04.
    {
      pattern: /^fal-ai\/flux-2\/lora(?:\/|$)|^fal-ai\/flux-2-lora-gallery\/|^fal-ai\/flux-2\/klein\/(?:4b|9b)(?:\/base)?(?:\/edit)?\/lora$/,
      reason: 'FLUX.2 LoRA endpoints (dev, the LoRA gallery, klein): custom LoRA weights, and litegen takes no LoRA input anywhere (2026-11-04)',
    },
    {
      id: 'fal-ai/flux-2/klein/realtime',
      reason: 'a realtime endpoint priced per compute second ("$0.00194 per compute seconds") with no queue or sync request to send (2026-11-04)',
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
    // FLUX.1 [pro] Kontext, ultra, Redux, fill and erase are carried since
    // 2026-11-04. These stay out:
    {
      id: 'fal-ai/flux-pro/v1/vto',
      reason: 'virtual try-on (a person image and a garment image): litegen has no try-on input (2026-11-04)',
    },
    {
      pattern: /^fal-ai\/flux-pro\/(?:v1\.1-ultra-finetuned|v1\/fill-finetuned)$/,
      reason: "BFL fine-tunes (a finetune_id and finetune_strength): litegen takes no fine-tuned weights (2026-11-04)",
    },
    {
      pattern:
        /^fal-ai\/(?:fast-fooocus-sdxl|fast-lightning-sdxl|fast-sdxl-controlnet-canny|sdxl-controlnet-union)(?:\/|$)|^fal-ai\/fast-sdxl\/|^fal-ai\/stable-diffusion-v(?:15|3-medium|35-large)(?:\/|$)/,
      reason: 'skipped 2026-09-11: SDXL and SD 1.5/3/3.5 variants',
    },
    // Recraft V2 (the listed fal-ai/recraft-20b), V3 with its image-to-image
    // twin, V4, V4.1, the V4 style endpoints, vectorize and upscale are carried
    // since 2026-11-04.
    {
      pattern: /^(?:recraft\/v4(?:\/pro)?|fal-ai\/recraft\/v3)\/create-style$/,
      reason: 'answers a style_id, not an image: the id goes to the carried style rows as extra.style_id (2026-11-04)',
    },
    {
      pattern:
        /^fal-ai\/ltx-2-19b\/|^fal-ai\/ltx-2\.3-(?:22b|quality)\/|^fal-ai\/ltx-2\.3\/(?:audio-to-video|extend-video|reframe|retake-video|(?:text|image)-to-video\/fast)$|^fal-ai\/ltx-video-13b-distilled\/(?:extend|multiconditioning)$|^fal-ai\/ltx-video-v095(?:\/|$)|^fal-ai\/ltxv-13b-098-distilled(?:\/|$)|^lightricks\/ltx-2\.5\//,
      reason: 'skipped 2026-09-11: the other LTX endpoints (fal/ltx-2.3 carries the LTX-2.3 Pro text/image-to-video pair)',
    },
  ],
};
