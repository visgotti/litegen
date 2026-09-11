// Replicate — an aggregator whose API needs a token, so the sources are its
// public, server-rendered model pages. "New" is scoped to the owners and
// families we carry:
//  - black-forest-labs flux*: the owner page lists every model the owner
//    publishes (no pagination).
//  - stability-ai sd* (stable-diffusion*, sdxl): the owner page, minus its
//    language and audio models.
//  - video: replicate/video is a community AnimateDiff build
//    (lucataco/animate-diff) pinned to one version, so its versions page is its
//    source. Replicate's text-to-video collection is a curated showcase, not a
//    listing, and the vendor families in it (Kling, Veo, Hailuo, …) are checked
//    through our direct providers for those vendors.
// Models the adapter calls at a pinned version are mapped to Replicate's
// `owner/name:version` reference, read from the model's versions page, so a
// pinned version that disappears upstream reports that model as missing. Those
// snapshots list the model's versions (a new version shows as "changed") plus
// the pinned version's input schema.
// Mapping mirrors litegen-core/src/providers/image/replicate.rs
// `resolve_model_version` and video/replicate.rs `resolve_model_version`
// (replicate/video is its AnimateDiff fallback arm).
import { openapiExcerpt } from '../extract.mjs';

const SDXL = { slug: 'stability-ai/sdxl', version: '39ed52f2a78e934b3ba6e2a89f5b1c712de7dfea535525255b1aa35c5565e08b' };
const ANIMATE_DIFF = { slug: 'lucataco/animate-diff', version: 'beecf59c4aee8d81bf04f0381033dfa10dc16e845b4ae00d281e2fa377e48a9f' };
const PINNED = [SDXL, ANIMATE_DIFF];

const ref = ({ slug, version }) => `${slug}:${version}`;

// `owner/name:version` for every version the page links to.
const versionRefs = (body, slug) =>
  [...body.matchAll(/href="\/([\w.-]+\/[\w.-]+)\/versions\/([0-9a-f]{64})"/g)].filter((m) => m[1] === slug).map((m) => `${m[1]}:${m[2]}`);

// The versions page embeds each version's Cog OpenAPI schema in React props.
function pinnedSchema(body, { slug, version }) {
  for (const m of body.matchAll(/<script id="react-component-props-[^"]*" type="application\/json">([\s\S]*?)<\/script>/g)) {
    let props;
    try {
      props = JSON.parse(m[1]);
    } catch {
      continue;
    }
    const doc = props?.version?.id === version ? props.version._extras?.dereferenced_openapi_schema : undefined;
    if (doc) return openapiExcerpt(doc, { paths: ['/predictions'], ours: [] });
  }
  return `pinned version ${slug}:${version} is not on this page\n`;
}

const versionsPage = (key, pin) => ({
  key,
  url: `https://replicate.com/${pin.slug}/versions`,
  expect: 'html',
  extract: (_html, body) => versionRefs(body, pin.slug),
  snapshot: (_html, body) =>
    `# versions\n${[...new Set(versionRefs(body, pin.slug))].sort().join('\n')}\n\n` +
    `# input schema of the pinned version ${pin.version.slice(0, 8)}\n${pinnedSchema(body, pin)}`,
});

export default {
  provider: 'replicate',
  models: {
    'replicate/flux-pro': 'black-forest-labs/flux-pro',
    'replicate/flux-1.1-pro': 'black-forest-labs/flux-1.1-pro',
    'replicate/flux-dev': 'black-forest-labs/flux-dev',
    'replicate/flux-schnell': 'black-forest-labs/flux-schnell',
    'replicate/sdxl': ref(SDXL),
    'replicate/sd3': 'stability-ai/stable-diffusion-3',
    'replicate/video': ref(ANIMATE_DIFF),
  },
  sources: [
    {
      key: 'black-forest-labs',
      url: 'https://replicate.com/black-forest-labs',
      expect: 'html',
      extract: /href="\/(black-forest-labs\/flux[\w.-]*)"/g,
    },
    {
      key: 'stability-ai',
      url: 'https://replicate.com/stability-ai',
      expect: 'html',
      extract: /href="\/(stability-ai\/(?:stable-diffusion|sdxl|sd\d)[\w.-]*)"/g,
    },
    versionsPage('sdxl-versions', SDXL),
    versionsPage('animate-diff-versions', ANIMATE_DIFF),
  ],
  acknowledged: [
    { id: SDXL.slug, reason: `carried at a pinned version — matched as ${SDXL.slug}:<version> through its versions page` },
    {
      pattern: new RegExp(`^(?:${PINNED.map((p) => p.slug.replace(/[.]/g, '\\.')).join('|')}):[0-9a-f]{64}$`),
      reason: 'another version of a model we pin to one version — a new version shows as a changed *-versions snapshot, not as a new model',
    },
    // Skips decided 2026-09-11 (docs/superpowers/2026-09-11-model-drift-decisions.md).
    // FLUX.1 ids are listed one by one so a new FLUX.1 model still surfaces.
    ...[
      'flux-1.1-pro-ultra',
      'flux-1.1-pro-ultra-finetuned',
      'flux-canny-dev',
      'flux-canny-pro',
      'flux-depth-dev',
      'flux-depth-pro',
      'flux-dev-lora',
      'flux-fill-dev',
      'flux-fill-pro',
      'flux-kontext-dev',
      'flux-kontext-dev-lora',
      'flux-kontext-max',
      'flux-kontext-pro',
      'flux-krea-dev',
      'flux-pro-finetuned',
      'flux-redux-dev',
      'flux-redux-schnell',
      'flux-schnell-lora',
      'flux-video-upscale',
    ].map((name) => ({
      id: `black-forest-labs/${name}`,
      reason: 'skipped 2026-09-11: another FLUX.1 model or FLUX tool; we carry flux-1.1-pro, flux-pro, flux-dev and flux-schnell here',
    })),
    {
      pattern: /^black-forest-labs\/flux-2-/,
      reason: 'skipped 2026-09-11: FLUX.2 on Replicate — BFL direct and fal cover it',
    },
    {
      id: 'black-forest-labs/flux-3',
      reason: "skipped 2026-09-11: FLUX 3 is a video model — decide together with BFL's own flux-3-video",
    },
    {
      pattern: /^stability-ai\/stable-diffusion-3\.5-/,
      reason: 'skipped 2026-09-11: the SD 3.5 family — Stability direct covers it',
    },
    ...['stable-diffusion', 'stable-diffusion-img2img', 'stable-diffusion-inpainting'].map((name) => ({
      id: `stability-ai/${name}`,
      reason: 'skipped 2026-09-11: an SD 1.5/2.1-era model',
    })),
  ],
};
