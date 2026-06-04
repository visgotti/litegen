/**
 * The infrastructure graph rendered by the "How it works" section.
 *
 * This module is the single source of truth for the diagram's *structure*:
 * which nodes exist, where they sit (as percentages within the stage), and the
 * provider metadata (media kind + model) shown on each provider node.
 *
 * Positions are stored for two layouts — wide (`pos`) and stacked (`posMobile`).
 * The DOM places nodes from these values; the WebGPU renderer then reads the
 * *actual* on-screen rectangles each frame, so the glowing links always follow
 * the labels regardless of which layout (or in-between size) is active.
 *
 * Translatable strings (pill labels, the "Image"/"Video" kind words) live in
 * `messages/<locale>.json`; only stable, non-translatable identifiers and brand
 * names live here.
 */

import { PROVIDER_CAPABILITIES } from './capabilities.generated';

export type NodeRole = 'app' | 'gateway' | 'provider';
export type MediaKind = 'image' | 'video';

/** Which input packet shapes a provider accepts (drives the streamed glyphs). */
export interface InputVocab {
  text: boolean;
  image: boolean;
  multi: boolean;
}

/** Which output modalities a provider produces (many do both). */
export interface OutputModalities {
  image: boolean;
  video: boolean;
}

/** A position within the stage, expressed in percent (0–100) of width/height. */
export interface Coord {
  x: number;
  y: number;
}

export interface FlowNode {
  id: string;
  role: NodeRole;
  /** Position in the wide / desktop layout. */
  pos: Coord;
  /** Position in the stacked / mobile layout. */
  posMobile: Coord;
  // Provider-only metadata (undefined for app/gateway):
  name?: string;
  /** Short 2-letter mark shown in the node when no logo is available. */
  initial?: string;
  /** Full-colour brand logo filename under public/logos/ (preferred over initial). */
  logo?: string;
  /**
   * Headline modality — drives the radial layout (image hemisphere up, video
   * down) and the node's halo tint. A provider may still *return* both kinds of
   * media; see `outputs`.
   */
  kind?: MediaKind;
  model?: string;
  /** Accepted input packet shapes (provider-only; attached from the registry). */
  inputs?: InputVocab;
  /** Output modalities produced (provider-only; many do both image + video). */
  outputs?: OutputModalities;
}

export const APP_NODE: FlowNode = {
  id: 'app',
  role: 'app',
  pos: { x: 7, y: 50 },
  posMobile: { x: 50, y: 6 },
};

export const GATEWAY_NODE: FlowNode = {
  id: 'gateway',
  role: 'gateway',
  pos: { x: 50, y: 50 },
  posMobile: { x: 50, y: 22 },
};

/**
 * Every integrated provider, shown as a node in the flow.
 *
 * The diagram is a *hub-and-spoke web*: the gateway sits dead-center, the prompt
 * (app) enters from due-left through a clear corridor, and the 18 providers are
 * *scattered* around the hub at varied radii and angles — deliberately NOT a
 * ring or a grid, so it reads as an organic mesh of services. Requests route
 * *through the middle* and media streams back out along every spoke.
 *
 * Desktop (`pos`) values are the baked output of a deterministic seeded layout
 * (mulberry32): each provider gets a random angle + elliptical radius, rejected
 * if it lands in the app→gateway corridor (x<48 & |y-50|<13), too near/far from
 * the hub, off-stage, or within ~13% of another node. Mobile (`posMobile`,
 * <=920px) is a staggered, jittered stack (alternating rows offset) — again not
 * a rigid grid. Regenerate both with the same seed if retuning; the renderer
 * reads each node's live on-screen rect, so the spokes follow wherever they land.
 *
 * Each provider may produce more than one modality: `kind` is the headline
 * (drives the halo tint); the Image/Video *tags* on the node and the kind of
 * media that streams back come from `outputs` (attached below). Node array order
 * is irrelevant to layout — the shader reads per-slot `kind`/flags, not position.
 */
const PROVIDER_NODES_RAW: FlowNode[] = [
  // Scattered around the hub (deterministic seeded jitter, not a ring/grid):
  // varied radius + angle per provider, with the left app→gateway corridor and a
  // minimum node separation respected. `kind` is the headline modality (halo
  // tint); the Image/Video tags shown on each node come from `outputs` below.
  { id: 'openai', role: 'provider', name: 'OpenAI', initial: 'OA', logo: 'openai.png', kind: 'image', pos: { x: 60.7, y: 87.7 }, posMobile: { x: 13, y: 37.3 } },
  { id: 'stability', role: 'provider', name: 'Stability', initial: 'St', logo: 'stability-ai.webp', kind: 'image', pos: { x: 21.4, y: 32.2 }, posMobile: { x: 40.9, y: 35.8 } },
  { id: 'replicate', role: 'provider', name: 'Replicate', initial: 'Re', logo: 'replicate.png', kind: 'image', pos: { x: 48.6, y: 21.6 }, posMobile: { x: 76.1, y: 36.8 } },
  { id: 'bfl', role: 'provider', name: 'BFL', initial: 'BF', logo: 'black-forest-labs.png', kind: 'image', pos: { x: 45, y: 88.9 }, posMobile: { x: 24.9, y: 45.3 } },
  { id: 'ideogram', role: 'provider', name: 'Ideogram', initial: 'Id', logo: 'ideogram.svg', kind: 'image', pos: { x: 21.2, y: 74 }, posMobile: { x: 54.5, y: 46.3 } },
  { id: 'recraft', role: 'provider', name: 'Recraft', initial: 'Rc', logo: 'recraft.png', kind: 'image', pos: { x: 82.5, y: 68.3 }, posMobile: { x: 85, y: 46.8 } },
  { id: 'leonardo', role: 'provider', name: 'Leonardo', initial: 'Le', logo: 'leonardo-ai.png', kind: 'image', pos: { x: 86.4, y: 52.9 }, posMobile: { x: 13, y: 54.5 } },
  { id: 'google', role: 'provider', name: 'Google', initial: 'Go', logo: 'google.png', kind: 'image', pos: { x: 58.5, y: 13.4 }, posMobile: { x: 42.4, y: 56.9 } },
  { id: 'fal', role: 'provider', name: 'Fal', initial: 'Fa', logo: 'fal.png', kind: 'image', pos: { x: 37.5, y: 14.1 }, posMobile: { x: 75, y: 54.6 } },
  { id: 'runway', role: 'provider', name: 'Runway', initial: 'Rw', logo: 'runway.png', kind: 'video', pos: { x: 77.9, y: 39.3 }, posMobile: { x: 23.4, y: 66.6 } },
  { id: 'luma', role: 'provider', name: 'Luma', initial: 'Lu', logo: 'luma.png', kind: 'video', pos: { x: 73.5, y: 23.2 }, posMobile: { x: 52.7, y: 64.1 } },
  { id: 'kling', role: 'provider', name: 'Kling', initial: 'Kl', logo: 'kling.png', kind: 'video', pos: { x: 29.2, y: 84.5 }, posMobile: { x: 83.2, y: 63.6 } },
  { id: 'minimax', role: 'provider', name: 'MiniMax', initial: 'MM', logo: 'minimax.png', kind: 'video', pos: { x: 40, y: 75.9 }, posMobile: { x: 13, y: 74.6 } },
  { id: 'bytedance', role: 'provider', name: 'ByteDance', initial: 'BD', logo: 'bytedance.svg', kind: 'video', pos: { x: 65, y: 72.7 }, posMobile: { x: 39.5, y: 76 } },
  { id: 'bedrock', role: 'provider', name: 'Bedrock', initial: 'Be', logo: 'amazon-bedrock.png', kind: 'video', pos: { x: 34.6, y: 28.3 }, posMobile: { x: 76.2, y: 74.5 } },
  { id: 'hunyuan', role: 'provider', name: 'Hunyuan', initial: 'Hy', logo: 'tencent-hunyuan.svg', kind: 'video', pos: { x: 78.7, y: 83 }, posMobile: { x: 23.3, y: 84.5 } },
  { id: 'vidu', role: 'provider', name: 'Vidu', initial: 'Vd', logo: 'vidu.png', kind: 'video', pos: { x: 89.4, y: 31.4 }, posMobile: { x: 60.9, y: 83.9 } },
  { id: 'pixverse', role: 'provider', name: 'PixVerse', initial: 'Px', logo: 'pixverse.png', kind: 'video', pos: { x: 73.6, y: 56.6 }, posMobile: { x: 87, y: 84.6 } },
];

/** Safe default for any provider missing from the generated snapshot. */
function vocabFor(id: string): InputVocab {
  return PROVIDER_CAPABILITIES[id]?.inputs ?? { text: true, image: true, multi: false };
}

/**
 * Output modalities for a provider. Falls back to the node's headline `kind`
 * when the provider is missing from the generated snapshot, so a spoke always
 * returns at least its own modality.
 */
function outputsFor(id: string, kind: MediaKind | undefined): OutputModalities {
  const o = PROVIDER_CAPABILITIES[id]?.outputs;
  if (o && (o.image || o.video)) return o;
  return { image: kind !== 'video', video: kind === 'video' };
}

/** Provider nodes with input vocabulary + output modalities attached from the registry. */
export const PROVIDER_NODES: FlowNode[] = PROVIDER_NODES_RAW.map((n) => ({
  ...n,
  inputs: vocabFor(n.id),
  outputs: outputsFor(n.id, n.kind),
}));

/** Render order MUST be app, gateway, providers — the shader relies on it. */
export const ALL_NODES: FlowNode[] = [APP_NODE, GATEWAY_NODE, ...PROVIDER_NODES];

/**
 * Capabilities shown as pills inside the gateway card. These are message keys
 * under `howItWorks.pills`; the gateway card resolves them to localized labels.
 */
// A curated six — the proxy value (routing / fallback / cost) plus what sets
// LiteGen apart from a text-LLM proxy: per-model capability *schemas*, key
// management, and completion *webhooks* for long-running video jobs. All are
// real (see README); rate-limiting and logging exist too but are left off the
// card to keep it from reading as a generic wall of proxy buzzwords.
export const GATEWAY_PILLS = [
  'routing',
  'fallback',
  'cost',
  'schemas',
  'keys',
  'webhooks',
] as const;

export type GatewayPill = (typeof GATEWAY_PILLS)[number];

/**
 * Pack a node's input vocabulary into the 3-bit mask the shader reads:
 * text = 1, image = 2, multi = 4. Returns 0 for nodes without a vocab
 * (app / gateway), which the shader treats as "text only".
 */
export function inputsMask(node: FlowNode): number {
  const v = node.inputs;
  if (!v) return 0;
  return (v.text ? 1 : 0) | (v.image ? 2 : 0) | (v.multi ? 4 : 0);
}

/**
 * Pack a node's output modalities into the 2-bit mask the shader reads to vary
 * the returning media tile: image = 1, video = 2 (a both-capable provider = 3
 * alternates image/video returns). Falls back to the headline `kind` for
 * app/gateway or providers without an `outputs` set.
 */
export function outputsMask(node: FlowNode): number {
  const o = node.outputs;
  if (!o) return node.kind === 'video' ? 2 : 1;
  return (o.image ? 1 : 0) | (o.video ? 2 : 0);
}
