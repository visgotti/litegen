/**
 * Pure helpers for the model-capability reference: turn a parsed `models/*.yaml`
 * model object into the slim `ModelEntry` shape the landing page renders, and
 * render the generated TS module. Kept dependency-free + side-effect-free so it
 * can be unit-tested (see derive-models.test.mjs).
 *
 * Derivation mirrors the Rust projection in litegen-core
 * (`project_model_info` / `extract_sizes`): sizes come from the `size` param
 * (enum mode), aspect ratios from `aspect_ratio.allowed`, max ref images from
 * `ref_inputs.max_total`. Each field the page shows is picked by name here, so
 * a field the schema gains reaches the page only once it is added below.
 *
 * Prices are the catalog's, never rounded. `base_cost_usd` is what the row's
 * own rules charge for its default request (litegen-core
 * `tests/catalog_conformance.rs`,
 * `every_base_price_is_what_the_rules_charge_for_the_default_request`), and
 * `variable_pricing` is carried rule by rule, in the order pricing.rs applies
 * it. A rule the page cannot word stops the derivation instead of vanishing.
 */

/**
 * Most-applicable developer/API documentation URL per provider: the fallback
 * reference link for a model whose YAML row declares no `docs_url` of its own
 * (see `referenceUrlOf`). Verified to resolve to docs (not marketing/pricing);
 * a few are the best-available official portal where the vendor has no clean
 * dedicated API-docs page.
 */
export const PROVIDER_DOCS = {
  openai: 'https://platform.openai.com/docs/guides/image-generation',
  google: 'https://ai.google.dev/gemini-api/docs',
  stability: 'https://platform.stability.ai/docs/api-reference',
  bfl: 'https://docs.bfl.ml/',
  ideogram: 'https://developer.ideogram.ai/',
  recraft: 'https://www.recraft.ai/docs/api-reference/getting-started',
  pixellab: 'https://api.pixellab.ai/v2/llms.txt',
  leonardo: 'https://docs.leonardo.ai/',
  runway: 'https://docs.dev.runwayml.com/',
  luma: 'https://docs.lumalabs.ai/docs/api',
  kling: 'https://app.klingai.com/global/dev/document-api/quickStart/productIntroduction/overview',
  minimax: 'https://platform.minimax.io/docs',
  bytedance: 'https://docs.byteplus.com/en/docs/ModelArk/',
  bedrock: 'https://docs.aws.amazon.com/bedrock/latest/userguide/model-parameters-stability-diffusion.html',
  hunyuan: 'https://www.tencentcloud.com/document/product/1284/74915',
  tokenhub: 'https://cloud.tencent.com/document/product/1823/130077',
  vidu: 'https://platform.vidu.com/docs/quick-start',
  pixverse: 'https://docs.platform.pixverse.ai/',
  replicate: 'https://replicate.com/docs/reference/http',
  fal: 'https://fal.ai/docs/documentation',
  tripo: 'https://developers.tripo3d.ai/en/docs/introduction',
  meshy: 'https://docs.meshy.ai/en/api/quick-start',
};

/**
 * A model's reference link: its own `docs_url` — the vendor's API page for the
 * exact endpoint it calls, which the catalog loader requires to be https —
 * else its provider's general docs.
 */
export function referenceUrlOf(m) {
  const own = typeof m.docs_url === 'string' ? m.docs_url : '';
  return own.startsWith('https://') ? own : PROVIDER_DOCS[m.provider];
}

// ─── Prices (litegen-core/src/capabilities/pricing.rs) ──────────────────────

/**
 * The effects a price rule may name, in the order `PriceRule::apply` tries
 * them (pricing.rs:147-165), each with the kind the page words it as. The
 * catalog loader allows exactly one per rule.
 */
const PRICE_EFFECTS = [
  ['usd', 'usd'],
  ['per_second_usd', 'perSecond'],
  ['per_megapixel_usd', 'perMegapixel'],
  ['per_item_usd', 'perItem'],
  ['multiply', 'multiply'],
  ['multiply_per_megapixel_above', 'multiplyPerMegapixelAbove'],
  ['add_usd', 'addUsd'],
];

/** The bounds a range condition may name (pricing.rs `RANGE_BOUNDS`). */
export const RANGE_BOUNDS = ['gt', 'gte', 'lt', 'lte'];

/** Facts `PriceFacts::new` sets for every request, sent or not. */
const ALWAYS_PRESENT_FACTS = ['duration_seconds', 'megapixels'];

/** Whether a param gives a price fact when the request leaves it out (pricing.rs `param_default`). */
function hasDefaultFact(spec) {
  if (!spec || spec.default === undefined || spec.default === null) return false;
  if (spec.kind === 'string_array') return Array.isArray(spec.default) && spec.default.length > 0;
  return ['bool', 'int', 'float', 'string'].includes(spec.kind);
}

/**
 * What one charge buys, the way litegen bills it: an image row per image it
 * returns (pricing.rs `request_price_usd`; proxy/router.rs charges per
 * delivered image), except a multiview set, which is one vendor call, and a
 * segmentation, priced by its row alone (`row_price_usd`); a video row per
 * video; a 3D row per mesh, generations and operations alike.
 */
export function priceUnitOf(m) {
  if (m.media_type === 'video') return 'video';
  if (m.media_type === 'model3d') return 'model3d';
  if (m.task === 'multiview' || m.task === 'segment') return 'request';
  if (m.params?.directions !== undefined) {
    // router.rs bills a request that sends `directions` as one call, and one
    // without it per image: the page has no single unit for that row.
    throw new Error(`${m.id}: a directions param makes the price per request or per image; the page has no unit for that`);
  }
  return 'image';
}

/** One `when` entry as the page words it: a value, `"*"` (sent), or a range. */
function priceConditionOf(fact, want, m) {
  if (want === '*') {
    // pricing.rs matches "*" on any fact present, and some always are.
    if (ALWAYS_PRESENT_FACTS.includes(fact) || hasDefaultFact(m.params?.[fact])) {
      throw new Error(`${m.id}: a price rule's "*" on ${fact} matches every request, so the page cannot word it as sent`);
    }
    return { fact, is: 'sent' };
  }
  if (['string', 'number', 'boolean'].includes(typeof want)) {
    // The page says a rule reads a param "as sent, else its default"; pricing.rs
    // keeps no default for some kinds (aspect_ratio), so a rule naming that
    // default would only apply when the request sends it.
    const spec = m.params?.[fact];
    if (spec && spec.default === want && !hasDefaultFact(spec)) {
      throw new Error(`${m.id}: a price rule's ${fact} = ${want} names the param's default, which pricing.rs does not apply to a request that leaves ${fact} out, so the page cannot word it`);
    }
    return { fact, is: 'eq', value: want };
  }
  if (want !== null && typeof want === 'object' && !Array.isArray(want)) {
    const bounds = Object.keys(want);
    if (bounds.length === 0 || bounds.some((b) => !RANGE_BOUNDS.includes(b) || typeof want[b] !== 'number')) {
      throw new Error(`${m.id}: a price rule's range for ${fact} must give a number for one or more of ${RANGE_BOUNDS.join(', ')}: ${JSON.stringify(want)}`);
    }
    const range = { fact, is: 'range' };
    for (const b of RANGE_BOUNDS) if (b in want) range[b] = want[b];
    return range;
  }
  throw new Error(`${m.id}: a price rule's condition on ${fact} is not a value, "*" or a range: ${JSON.stringify(want)}`);
}

/** One `variable_pricing` rule: its conditions (none = always) and its one effect. */
function priceRuleOf(rule, m) {
  const fields = rule !== null && typeof rule === 'object' ? Object.keys(rule) : [];
  const known = new Set(['when', 'per_item', ...PRICE_EFFECTS.map(([f]) => f)]);
  const effects = PRICE_EFFECTS.filter(([f]) => rule?.[f] !== undefined && rule?.[f] !== null);
  if (
    fields.length === 0 ||
    fields.some((f) => !known.has(f)) ||
    effects.length !== 1 ||
    typeof rule[effects[0][0]] !== 'number' ||
    (effects[0][1] === 'perItem') !== (typeof rule.per_item === 'string')
  ) {
    throw new Error(`${m.id}: a price rule needs exactly one effect the page can word: ${JSON.stringify(rule)}`);
  }
  const when = rule.when ?? {};
  if (when === null || typeof when !== 'object' || Array.isArray(when)) {
    throw new Error(`${m.id}: a price rule's when must map facts to values: ${JSON.stringify(rule)}`);
  }
  const [field, kind] = effects[0];
  const effect = kind === 'perItem' ? { kind, value: rule[field], list: rule.per_item } : { kind, value: rule[field] };
  return { when: Object.entries(when).map(([fact, want]) => priceConditionOf(fact, want, m)), effect };
}

/** The row's price: the exact catalog amount, its unit, and its rules in order. */
function pricingOf(m) {
  if (m.pricing?.base_cost_usd === undefined) return undefined;
  return {
    baseCostUsd: m.pricing.base_cost_usd,
    unit: priceUnitOf(m),
    rules: (m.pricing.variable_pricing ?? []).map((rule) => priceRuleOf(rule, m)),
  };
}

// ─── What a model takes (litegen-core/src/capabilities/schema.rs) ───────────

/** Param kinds whose `max_items` is how many entries one request may send. */
const COUNTED_LISTS = ['layout', 'segmentation_prompts', 'animation_list'];

/** Copy each `[yamlField, pageField]` of `src` that is set (not null or absent) into `dst`. */
function copySet(dst, src, fields) {
  for (const [from, to] of fields) {
    const v = src?.[from];
    if (v !== undefined && v !== null) dst[to] = v;
  }
  return dst;
}

/** A YAML date as the catalog writes it (YYYY-MM-DD), whether the parser kept it a string or not. */
function dayOf(v) {
  return v instanceof Date ? v.toISOString().slice(0, 10) : v;
}

/**
 * The vendor's lifecycle dates and successor, word for word, or undefined.
 * No status is computed from today's date: that would make the generated
 * snapshot change by itself (CI checks it is unchanged) and the static HTML
 * disagree with hydration. So between a row's `retires_at` and its removal
 * from the YAML (catalog_conformance.rs makes that removal due), the page
 * still lists a model litegen stops serving the day after that date
 * (schema.rs `status_on`).
 */
function lifecycleOf(m) {
  const life = copySet({}, { deprecated_at: dayOf(m.deprecated_at), retires_at: dayOf(m.retires_at), replaced_by: m.replaced_by }, [
    ['deprecated_at', 'deprecatedAt'],
    ['retires_at', 'retiresAt'],
    ['replaced_by', 'replacedBy'],
  ]);
  return Object.keys(life).length > 0 ? life : undefined;
}

/** A freeform size's bounds (schema.rs `SizeSpecFreeform`), or undefined for a listed size. */
function sizeRangeOf(size) {
  if (size?.mode !== 'freeform') return undefined;
  return copySet({}, size, [
    ['min_width', 'minWidth'],
    ['max_width', 'maxWidth'],
    ['min_height', 'minHeight'],
    ['max_height', 'maxHeight'],
    ['multiple_of', 'multipleOf'],
    ['min_pixels', 'minPixels'],
    ['max_pixels', 'maxPixels'],
    ['max_aspect', 'maxAspect'],
  ]);
}

/** The roles validator.rs `check_frame_shape` treats as a frame image. */
const FRAME_ROLES = ['first_frame', 'last_frame', 'init'];

/** The `frames_set_shape` values the page words (schema.rs `FramesSetShape`). */
const FRAMES_SET_SHAPES = ['always', 'without_references'];

/**
 * When a frame image sets a video's shape (the aspect ratio's
 * `frames_set_shape`), as validator.rs `check_frame_shape` applies it: an
 * aspect_ratio sent with an image in one of the row's frame roles is refused
 * (strict) or dropped and named (lax), unless the row is `without_references`
 * and a `reference` image goes with it. A role-less image has taken the row's
 * `default_role` by then, so it counts when that role is a frame role.
 * Undefined when the row has no such rule or takes no frame image.
 */
function aspectRatioFramesOf(m) {
  const shape = m.params?.aspect_ratio?.frames_set_shape;
  if (shape === undefined || shape === null) return undefined;
  if (!FRAMES_SET_SHAPES.includes(shape)) {
    throw new Error(`${m.id}: aspect_ratio.frames_set_shape ${JSON.stringify(shape)} is not one the page can word (${FRAMES_SET_SHAPES.join(', ')})`);
  }
  const taken = Object.keys(m.ref_inputs?.roles ?? {});
  const roles = FRAME_ROLES.filter((r) => taken.includes(r));
  if (roles.length === 0) return undefined;
  const frames = { roles, unlessReference: shape === 'without_references' && taken.includes('reference') };
  const defaultRole = m.ref_inputs?.default_role;
  if (typeof defaultRole === 'string' && roles.includes(defaultRole)) frames.defaultRole = defaultRole;
  return frames;
}

/**
 * What a caller must know about one param beyond its kind, range and default:
 * the only values a number takes, its words, and the limits of its kind.
 */
function paramContractOf(p) {
  const out = {};
  // The validator takes only these values, so the page shows them instead of the range.
  if ((p?.kind === 'int' || p?.kind === 'float') && Array.isArray(p.values) && p.values.length > 0) out.values = p.values.map(Number);
  copySet(out, p, [['label', 'label'], ['description', 'description']]);
  if (p?.kind === 'string') copySet(out, p, [['max_length', 'maxLength']]);
  // For a string_array (output_formats) max_items counts vendor jobs, not members (schema.rs), so it is not "up to N".
  if (COUNTED_LISTS.includes(p?.kind)) copySet(out, p, [['max_items', 'maxItems']]);
  switch (p?.kind) {
    case 'source_model':
      copySet(out, p, [['formats', 'formats'], ['max_bytes', 'maxBytes'], ['max_faces', 'maxFaces']]);
      if (p.requires_skin === true) out.requiresSkin = true;
      if (p.requires_texture === true) out.requiresTexture = true;
      break;
    case 'animation_list':
      // A request names a clip by the vendor's id (or the canonical preset it plays).
      out.presets = (p.presets ?? []).map((preset) => copySet({ id: String(preset.id) }, preset, [['label', 'label'], ['canonical', 'canonical']]));
      break;
    case 'palette':
      copySet(out, p, [['max_colors', 'maxColors']]);
      if (p.forced === true) out.forced = true;
      break;
    case 'segmentation_prompts':
      copySet(out, p, [['max_masks', 'maxMasks']]);
      break;
    case 'expand':
      copySet(out, p, [['max_px', 'maxPx']]);
      break;
    case 'layout':
      if (p.single_word_text === true) out.singleWordText = true;
      break;
  }
  return out;
}

/**
 * The badges that say what a model does, in the order the page shows them,
 * read as GET /v1/models reads them (`project_model_info`). An operation on a
 * source (rig, animate, segment) gets none: its task badge says what it does.
 */
export function modalitiesOf(m) {
  const caps = m.capabilities ?? {};
  const roles = m.ref_inputs?.roles ?? {};
  const out = [];
  if (caps.text_to_image === true) out.push('textToImage');
  if (caps.image_to_image === true) out.push('imageToImage');
  // "Takes a mask", as the API's supports_inpainting reads it.
  if ('mask' in roles) out.push('inpainting');
  if (caps.text_to_video === true) out.push('textToVideo');
  if (caps.image_to_video === true) out.push('imageToVideo');
  if (caps.text_to_3d === true) out.push('textTo3d');
  if (caps.image_to_3d === true) out.push('imageTo3d');
  if (caps.multiview_to_3d === true) out.push('multiviewTo3d');
  if (caps.background_removal === true) out.push('backgroundRemoval');
  if (caps.native_pixel_art === true) out.push('pixelArt');
  return out;
}

/**
 * What a model works on in place of a prompt: a source mesh or clip, or the
 * image it segments. Read by param name and kind, as the API's
 * supports_source_model, supports_source_video and supports_segmentation are.
 */
function worksOnOf(m) {
  const params = m.params ?? {};
  if (params.source_model?.kind === 'source_model') return 'sourceModel';
  if (params.source_video?.kind === 'source_video') return 'sourceVideo';
  if (m.task === 'segment' && params.prompts?.kind === 'segmentation_prompts') return 'segmentation';
  return undefined;
}

/** Map one parsed YAML model to a ModelEntry. Defensive about missing fields. */
export function deriveModel(m) {
  const params = m.params ?? {};

  const sizeParam = params.size;
  const sizes =
    sizeParam && sizeParam.mode === 'enum' && Array.isArray(sizeParam.values)
      ? sizeParam.values.map(([w, h]) => `${w}x${h}`)
      : [];
  const sizeRange = sizeRangeOf(sizeParam);

  const arParam = params.aspect_ratio;
  const aspectRatios =
    arParam && Array.isArray(arParam.allowed) ? arParam.allowed.slice() : [];

  const ri = m.ref_inputs;
  const refRoles = ri && ri.roles
    ? Object.entries(ri.roles).map(([name, r]) => ({
        name,
        required: r?.required === true,
        min: r?.min_count ?? 0,
        max: r?.max_count ?? 0,
      }))
    : [];

  // Display params: everything except size/aspect_ratio (shown separately).
  const displayParams = Object.entries(params)
    .filter(([name]) => name !== 'size' && name !== 'aspect_ratio')
    .map(([name, p]) => {
      // For a 3D model's `output_formats` this is what the VENDOR emits. The
      // API serves more (every container litegen converts to locally), but that
      // set is the gateway's to compute and grows as it learns new containers,
      // so the page says so (ModelsReference) rather than copying the rule here.
      // A segmentation's prompt types and a layout's box kinds are its choices too.
      const choices = p?.enum_values ?? (p?.kind === 'segmentation_prompts' ? p.types : p?.kind === 'layout' ? p.kinds : undefined);
      const enumValues = Array.isArray(choices) ? choices.map(String) : undefined;
      return {
      name,
      kind: p?.kind ?? 'string',
      enum: enumValues,
      default: p?.default !== undefined ? p.default : undefined,
      min: p?.min !== undefined ? p.min : undefined,
      max: p?.max !== undefined ? p.max : undefined,
      ...paramContractOf(p),
      };
    });

  const caps = m.capabilities ?? {};
  const prompt = m.prompt ?? {};
  const lifecycle = lifecycleOf(m);
  const worksOn = worksOnOf(m);
  const aspectRatioFrames = aspectRatioFramesOf(m);

  return {
    id: m.id,
    provider: m.provider,
    displayName: m.display_name ?? m.id,
    description: m.description ?? '',
    mediaType: m.media_type,
    output: m.media_type,
    // What a request to it does: generate, or an operation on a source
    // (rig, animate, texture, extend, segment …); the YAML default is generate.
    task: typeof m.task === 'string' ? m.task : 'generate',
    capabilities: {
      textToImage: caps.text_to_image === true,
      imageToImage: caps.image_to_image === true,
      inpainting: caps.inpainting === true,
      textToVideo: caps.text_to_video === true,
      imageToVideo: caps.image_to_video === true,
      textTo3d: caps.text_to_3d === true,
      imageTo3d: caps.image_to_3d === true,
      multiviewTo3d: caps.multiview_to_3d === true,
      backgroundRemoval: caps.background_removal === true,
      nativePixelArt: caps.native_pixel_art === true,
    },
    modalities: modalitiesOf(m),
    ...(worksOn ? { worksOn } : {}),
    ...(lifecycle ? { lifecycle } : {}),
    sizes,
    ...(sizeRange ? { sizeRange } : {}),
    ...copySet({}, sizeParam, [['label', 'sizeLabel'], ['description', 'sizeDescription']]),
    aspectRatios,
    ...copySet({}, arParam, [['label', 'aspectRatioLabel'], ['description', 'aspectRatioDescription']]),
    ...(aspectRatioFrames ? { aspectRatioFrames } : {}),
    maxRefImages: ri?.max_total ?? 0,
    refRoles,
    promptLimits: {
      required: prompt.required !== false,
      minLength: prompt.min_length,
      maxLength: prompt.max_length,
    },
    params: displayParams,
    pricing: pricingOf(m),
    tags: Array.isArray(m.tags) ? m.tags.slice() : [],
    referenceUrl: referenceUrlOf(m),
  };
}

/** Derive + sort a list of YAML models for stable generated output. */
export function deriveModels(yamlModels) {
  return yamlModels.map(deriveModel).sort((a, b) => a.id.localeCompare(b.id));
}

const HEADER = `// GENERATED by scripts/sync-models.mjs — do not edit by hand.
// Run \`npm run sync-models\` to refresh from the repo's models/*.yaml.`;

const INTERFACE = `export interface ModelParam {
  name: string;
  kind: string;
  /** The choices: enum values, a segmentation's prompt types, a layout's box kinds. */
  enum?: string[];
  /** An array for a set-valued param (\`output_formats\`). */
  default?: string | number | boolean | string[];
  min?: number;
  max?: number;
  /** The only values a number param takes; shown instead of the range. */
  values?: number[];
  label?: string;
  description?: string;
  /** The longest string, in characters. */
  maxLength?: number;
  /** The most entries one request may send (layout boxes, segmentation prompts, clips). */
  maxItems?: number;
  /** A source mesh: the containers the vendor takes, its size and triangle limits, what it must have. */
  formats?: string[];
  maxBytes?: number;
  /** The most triangles (\`max_faces\`; litegen counts a quad as two). */
  maxFaces?: number;
  requiresSkin?: boolean;
  requiresTexture?: boolean;
  /** An animate model's clips, named by the vendor's id or the canonical preset each plays. */
  presets?: { id: string; label?: string; canonical?: string }[];
  /** A palette: the most colours, and whether every pixel must be one of them. */
  maxColors?: number;
  forced?: boolean;
  /** A segmentation: the most masks one answer holds. */
  maxMasks?: number;
  /** An expand: the most pixels added to one side. */
  maxPx?: number;
  /** A layout: each text box holds one word. */
  singleWordText?: boolean;
}

/** A freeform size's bounds, in pixels. */
export interface SizeRange {
  minWidth: number;
  maxWidth: number;
  minHeight: number;
  maxHeight: number;
  multipleOf?: number;
  minPixels?: number;
  maxPixels?: number;
  /** The widest long-to-short side ratio. */
  maxAspect?: number;
}

/** A badge that says what a model does. */
export type ModalityKey =
  | 'textToImage'
  | 'imageToImage'
  | 'inpainting'
  | 'textToVideo'
  | 'imageToVideo'
  | 'textTo3d'
  | 'imageTo3d'
  | 'multiviewTo3d'
  | 'backgroundRemoval'
  | 'pixelArt';

/**
 * A video whose frame image sets its shape (the aspect ratio's
 * \`frames_set_shape\`): a ratio applies only without an image in one of these
 * roles, or, when \`unlessReference\`, with a \`reference\` image beside it.
 */
export interface AspectRatioFrames {
  /** The row's frame roles (first_frame, last_frame, init). */
  roles: string[];
  /** A \`reference\` image beside the frame lifts the rule. */
  unlessReference: boolean;
  /** The role an image sent without one takes, when it is a frame role. */
  defaultRole?: string;
}

/** The vendor's lifecycle dates (YYYY-MM-DD) and the catalog model to move to. */
export interface ModelLifecycle {
  deprecatedAt?: string;
  retiresAt?: string;
  replacedBy?: string;
}

export interface ModelRefRole {
  name: string;
  required: boolean;
  min: number;
  max: number;
}

/**
 * One condition of a price rule: a fact as the request sends it (else its
 * default) equal to a value, sent at all (\`"*"\`), or inside a range.
 */
export type PriceCondition =
  | { fact: string; is: 'eq'; value: string | number | boolean }
  | { fact: string; is: 'sent' }
  | { fact: string; is: 'range'; gt?: number; gte?: number; lt?: number; lte?: number };

/** A rule's one effect: a price or a rate replaces the price so far; the rest adjust it. */
export type PriceEffect =
  | { kind: 'usd' | 'perSecond' | 'perMegapixel' | 'multiply' | 'multiplyPerMegapixelAbove' | 'addUsd'; value: number }
  | { kind: 'perItem'; value: number; list: string };

export interface PriceRuleEntry {
  /** Every condition must hold; none means the rule always applies. */
  when: PriceCondition[];
  effect: PriceEffect;
}

export interface ModelPricing {
  /** The vendor's price for the default request, exactly as the catalog gives it. */
  baseCostUsd: number;
  /** What one charge buys. */
  unit: 'image' | 'video' | 'model3d' | 'request';
  /** Applied in order over baseCostUsd for the request at hand; the result never goes below 0. */
  rules: PriceRuleEntry[];
}

export interface ModelEntry {
  id: string;
  provider: string;
  displayName: string;
  description: string;
  mediaType: 'image' | 'video' | 'model3d';
  output: 'image' | 'video' | 'model3d';
  /** generate, or the operation the model performs on a source. */
  task: string;
  capabilities: {
    textToImage: boolean;
    imageToImage: boolean;
    inpainting: boolean;
    textToVideo: boolean;
    imageToVideo: boolean;
    textTo3d: boolean;
    imageTo3d: boolean;
    multiviewTo3d: boolean;
    backgroundRemoval: boolean;
    nativePixelArt: boolean;
  };
  /** The badges that say what the model does (messages \`reference.modality.*\`). */
  modalities: ModalityKey[];
  /** What it works on in place of a prompt: a source mesh or clip, or the image it segments. */
  worksOn?: 'sourceModel' | 'sourceVideo' | 'segmentation';
  lifecycle?: ModelLifecycle;
  sizes: string[];
  /** A freeform size: any width and height inside these bounds. */
  sizeRange?: SizeRange;
  sizeLabel?: string;
  sizeDescription?: string;
  aspectRatios: string[];
  aspectRatioLabel?: string;
  aspectRatioDescription?: string;
  /** Set when a frame image sets the shape instead of a ratio. */
  aspectRatioFrames?: AspectRatioFrames;
  maxRefImages: number;
  refRoles: ModelRefRole[];
  promptLimits: { required: boolean; minLength?: number; maxLength?: number };
  params: ModelParam[];
  pricing?: ModelPricing;
  tags: string[];
  /** Most-applicable provider documentation URL. */
  referenceUrl?: string;
}`;

/** Render the generated TS module. The data is plain JSON, valid as a TS literal. */
export function renderGeneratedTs(models) {
  return `${HEADER}
${INTERFACE}

export const MODELS: ModelEntry[] = ${JSON.stringify(models, null, 2)};
`;
}

/**
 * Compact per-provider model index (display names grouped by output modality) —
 * a small module the diagram imports for its node detail panels, so the homepage
 * doesn't pull the full MODELS catalog.
 */
export function buildProviderModels(models) {
  const out = {};
  for (const m of models) {
    const p = (out[m.provider] ??= { image: [], video: [], model3d: [] });
    const name = m.displayName || m.id;
    if (m.output === 'video') p.video.push(name);
    else if (m.output === 'model3d') p.model3d.push(name);
    else p.image.push(name);
  }
  return out;
}

export function renderProviderModelsTs(models) {
  const map = buildProviderModels(models);
  const lines = Object.keys(map)
    .sort()
    .map((id) => `  ${JSON.stringify(id)}: ${JSON.stringify(map[id])},`);
  return `// GENERATED by scripts/sync-models.mjs — do not edit by hand.
// Per-provider model lists for the InfraFlow node detail panels.
export interface ProviderModels {
  image: string[];
  video: string[];
  model3d: string[];
}

export const PROVIDER_MODELS: Record<string, ProviderModels> = {
${lines.join('\n')}
};
`;
}
