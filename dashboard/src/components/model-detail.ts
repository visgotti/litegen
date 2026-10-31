/**
 * Pure helpers behind the model detail drawer, kept out of the component so
 * they are unit-testable without a DOM and so the component module exports
 * only components (react-refresh requires that for hot reload).
 */
import type { ModelInfo, ModelSchema } from '@litegen/sdk';
import { allShareOneJob, derivableValues, promptUsage, roleLabel } from '../playground/params';

/** Display name for every `ModelCapabilities` boolean flag, in the order the
 *  chips are rendered. Keep this in step with the schema: a flag with no entry
 *  here is invisible in the UI, and a model whose flags are ALL missing renders
 *  as '—', which reads as "no capabilities" rather than "not labelled yet". */
const CAP_LABELS: Record<string, string> = {
  supports_text_to_image: 'Text → image',
  supports_image_to_image: 'Image → image',
  supports_inpainting: 'Inpainting',
  supports_text_to_video: 'Text → video',
  supports_image_to_video: 'Image → video',
  supports_first_frame: 'First-frame ref',
  supports_last_frame: 'Last-frame ref',
  supports_text_to_3d: 'Text → 3D',
  supports_image_to_3d: 'Image → 3D',
  supports_multiview_to_3d: 'Multiview → 3D',
  supports_texture: 'Textures',
  supports_pbr: 'PBR materials',
  supports_rig: 'Auto-rigging',
  supports_animation: 'Animation clips',
  supports_skeleton: 'Skeleton choice',
  supports_joint_naming: 'Joint naming',
  supports_source_model: 'Reads a source mesh',
  supports_keep_uvs: 'Keeps UVs',
  supports_placement: 'Placement',
  supports_keyframes: 'Timed keyframes',
  supports_fixed_camera: 'Fixed camera',
  supports_audio_toggle: 'Audio on/off',
  supports_loop: 'Loop',
  supports_source_video: 'Reads a source clip',
  supports_time_range: 'Time range',
  supports_region_edit: 'Region edit',
  supports_mask_prompt: 'Mask by prompt',
  supports_preserve_unmasked: 'Keep unmasked pixels',
  supports_mask_grow: 'Mask growth',
  supports_expand: 'Expand canvas',
  supports_layout: 'Layout boxes',
  supports_segmentation: 'Segmentation',
  supports_background_removal: 'Background removal',
  supports_transparent_background: 'Transparent background',
  supports_palette: 'Palette',
  supports_forced_palette: 'Forced palette',
  native_pixel_art: 'Native pixel art',
  lossy_only: 'Lossy output only',
};

type Caps = Record<string, unknown>;
const listOf = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/**
 * The model's non-boolean catalog fields as label/value rows: its `task`,
 * then what shapes a request — keyframes, durations, the region and mask
 * conventions, texture resolutions, layout and prompt kinds — and when it
 * retires. Fields the model does not report are left out.
 */
export function catalogFacts(model: ModelInfo): Array<[string, string]> {
  const c = (model.capabilities ?? {}) as Caps;
  const rows: Array<[string, string]> = [];
  const add = (label: string, value: unknown) => {
    if (value != null && value !== '') rows.push([label, String(value)]);
  };
  add('Task', c.task);
  if (c.max_keyframes != null) {
    add('Keyframes', `up to ${String(c.max_keyframes)}${c.keyframe_timing ? ` · ${String(c.keyframe_timing)}` : ''}`);
  }
  const durations = listOf(c.duration_values);
  if (durations.length) add('Durations', durations.map(d => `${String(d)} s`).join(', '));
  add('Region input', c.region_input);
  add('Mask convention', c.mask_convention);
  add('Unmasked pixels', c.preserves_unmasked);
  const textures = listOf(c.texture_resolutions);
  if (textures.length) add('Texture resolutions', `${textures.join(', ')} px`);
  const layout = listOf(c.layout_kinds);
  if (layout.length) add('Layout boxes', layout.join(', '));
  const prompts = listOf(c.segmentation_prompts);
  if (prompts.length) add('Segmentation prompts', prompts.join(', '));
  const directions = listOf(c.directions);
  if (directions.length) add('Directions', directions.join(', '));
  if (c.pixel_size != null) add('Pixel size', typeof c.pixel_size === 'object' ? JSON.stringify(c.pixel_size) : c.pixel_size);
  add('Extend output', c.extend_output);
  if (model.deprecated_at) add('Deprecated', model.deprecated_at);
  if (model.retires_at) add('Retires', `${model.retires_at}${model.replaced_by ? ` → ${model.replaced_by}` : ''}`);
  return rows;
}

/** Labels for the capability flags a model reports as true, CAP_LABELS order. */
export function capabilityLabels(capabilities: unknown): string[] {
  const flags = (capabilities ?? {}) as Record<string, unknown>;
  return Object.entries(CAP_LABELS).filter(([k]) => flags[k]).map(([, label]) => label);
}

type AnySpec = { kind?: string; [k: string]: unknown };

/** Compact one-line summary of a param spec (kind + allowed/range + default). */
export function describeSpec(spec: AnySpec): string {
  const kind = spec.kind ?? 'value';
  const strs = (v: unknown): string[] => listOf(v).filter((x): x is string => typeof x === 'string');
  switch (kind) {
    case 'region':
      return `region: a ${String(spec.input ?? 'box')}, in pixels of the input image`
        + (spec.min_window_px != null ? ` · at least ${String(spec.min_window_px)} px` : '');
    case 'animation_list': {
      const presets = listOf(spec.presets).map(p => (p as { id?: unknown }).id).filter((id): id is string => typeof id === 'string');
      return `animation_list: up to ${String(spec.max_items ?? '')} clips`
        + (presets.length ? ` · presets ${presets.join(', ')}` : '')
        + (spec.text_to_motion != null ? ' · or from a prompt' : '');
    }
    case 'source_model':
    case 'source_video':
      return `${kind}: a ${strs(spec.formats).join('/') || 'file'}, by generation id or URL`;
    case 'palette':
      return `palette: up to ${String(spec.max_colors ?? '')} colours${spec.forced === true ? ' (forced)' : ''}`;
    case 'layout':
      return `layout: up to ${String(spec.max_items ?? '')} boxes · ${strs(spec.kinds).join(', ')}`;
    case 'expand':
      return `expand: pixels per side${spec.max_px != null ? `, up to ${String(spec.max_px)}` : ''}${spec.centered_only === true ? ' · centred only' : ''}`;
    case 'time_range':
      return `time_range: start_s–end_s${spec.min_window_s != null ? ` · at least ${String(spec.min_window_s)} s` : ''}`;
    case 'segmentation_prompts':
      return `segmentation_prompts: ${strs(spec.types).join(', ')} · up to ${String(spec.max_items ?? '')}`;
  }
  if (kind === 'string_array') {
    // `max_items` counts provider jobs, and formats in `derivable_values` share
    // one — say so, or "glb | obj | stl | ply · max 1" reads as "pick one".
    const values = Array.isArray(spec.enum_values) ? (spec.enum_values as string[]) : [];
    const max = typeof spec.max_items === 'number' ? spec.max_items : null;
    const shared = derivableValues(spec as { kind: string });
    let budget = '';
    if (allShareOneJob(spec as { kind: string })) {
      // The common case since litegen converts between all its containers:
      // the cap cannot bind, so say what that means rather than list it all.
      budget = ' · any combination is one provider job';
    } else if (max != null) {
      budget = ` · ${max} provider job${max === 1 ? '' : 's'}`;
      if (shared.length > 1) budget += ` (${shared.join('/')} share one)`;
    }
    return `${kind}: ${values.join(' | ')}${budget}`;
  }
  const allowed = (spec.allowed as string[] | undefined) ?? (spec.enum_values as string[] | undefined);
  if (Array.isArray(allowed) && allowed.length) return `${kind}: ${allowed.join(' | ')}`;
  if (spec.min != null || spec.max != null) return `${kind} ${spec.min ?? ''}–${spec.max ?? ''}`;
  if (spec.default != null) return `${kind} (default ${String(spec.default)})`;
  return kind;
}

/** How the drawer states the model's prompt rule. */
export function describePrompt(schema: ModelSchema): string {
  const max = (schema as unknown as { prompt?: { max_length?: number | null } }).prompt?.max_length;
  const limit = typeof max === 'number' ? ` · up to ${max.toLocaleString()} characters` : '';
  switch (promptUsage(schema)) {
    case 'unused': return 'Not used — this model generates from the input image';
    case 'optional': return `Optional when a reference image is attached${limit}`;
    default: return `Required${limit}`;
  }
}

export interface RefRoleRow {
  role: string;
  label: string;
  /** e.g. "required · exactly 1", "optional · up to 5". */
  rule: string;
}

/** One row per reference-image role the model accepts, in a readable form. */
export function refRoleRows(schema: ModelSchema | null | undefined): RefRoleRow[] {
  const roles = (schema as unknown as { ref_inputs?: { roles?: Record<string, { required?: boolean; min_count?: number; max_count?: number }> } } | null | undefined)
    ?.ref_inputs?.roles ?? {};
  return Object.entries(roles).map(([role, r]) => {
    const min = r.min_count ?? 0;
    const max = r.max_count ?? 0;
    const required = r.required === true && min > 0;
    const count = min === max ? `exactly ${max}` : min > 0 ? `${min}–${max}` : `up to ${max}`;
    return { role, label: roleLabel(role), rule: `${required ? 'required' : 'optional'} · ${count}` };
  });
}

/** Placeholder input the curl example uses for a required reference image. */
export const EXAMPLE_IMAGE_URL = 'https://example.com/input.png';
/** What a copied segmentation call carries: the image itself, since
 *  `/v1/images/segmentations` takes "base64, or a `data:` URI", not a URL. */
export const EXAMPLE_IMAGE_BASE64 = '<base64 PNG, JPEG, WebP or GIF, or a data: URI>';

/**
 * A request body that would pass this model's validation — what "Copy as curl"
 * should hand someone. A required image role gets placeholder URLs (an
 * image-only model rejects a prompt-only request), and a model that never
 * reads the prompt is not shown one.
 */
export function exampleBody(model: ModelInfo, schema: ModelSchema | null | undefined): Record<string, unknown> {
  const caps = (model.capabilities ?? {}) as Caps;
  if (caps.task === 'segment') {
    // `/v1/images/segmentations`: an image and what to select in it.
    const kind = listOf(caps.segmentation_prompts)[0] ?? 'point';
    const prompt = kind === 'text' ? { type: 'text', text: 'the main object' }
      : kind === 'box' ? { type: 'box', x: 0, y: 0, w: 256, h: 256 }
        : { type: 'point', x: 256, y: 256 };
    return { model: model.id, image: EXAMPLE_IMAGE_BASE64, prompts: [prompt] };
  }
  const body: Record<string, unknown> = { model: model.id };
  const usage = schema ? promptUsage(schema) : 'required';
  if (usage !== 'unused') {
    body.prompt = model.media_type === 'model3d' ? 'a weathered bronze statue of a fox' : 'a photo of a cat';
  }
  if (model.media_type === 'image') body.n = 1;
  const refs: Array<{ type: 'url'; value: string; role: string }> = [];
  const roles = (schema as unknown as { ref_inputs?: { roles?: Record<string, { required?: boolean; min_count?: number }> } } | null | undefined)
    ?.ref_inputs?.roles ?? {};
  for (const [role, r] of Object.entries(roles)) {
    const min = r.min_count ?? 0;
    if (r.required === true && min > 0) {
      for (let i = 0; i < min; i++) refs.push({ type: 'url', value: EXAMPLE_IMAGE_URL, role });
    }
  }
  if (refs.length) body.reference_images = refs;
  // An operation reads a source by generation id (`GET /v1/generations/{id}`).
  const params = ((schema as unknown as { params?: Record<string, AnySpec> } | null | undefined)?.params) ?? {};
  for (const [name, spec] of Object.entries(params)) {
    if (spec.kind === 'source_model' || spec.kind === 'source_video') body[name] = { type: 'generation', value: '<generation id>' };
  }
  return body;
}

/** The family endpoint a model's generations go to. */
export function generationEndpoint(model: ModelInfo): string {
  if ((model.capabilities as Caps | undefined)?.task === 'segment') return '/v1/images/segmentations';
  return model.media_type === 'model3d' ? '/v1/models3d/generations'
    : model.media_type === 'video' ? '/v1/videos/generations'
    : '/v1/images/generations';
}

export function buildCurl(model: ModelInfo, schema?: ModelSchema | null): string {
  return [
    `curl -X POST $LITEGEN_BASE${generationEndpoint(model)} \\`,
    `  -H "Authorization: Bearer $LITEGEN_KEY" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '${JSON.stringify(exampleBody(model, schema))}'`,
  ].join('\n');
}
