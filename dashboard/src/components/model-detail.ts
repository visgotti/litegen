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
};

/** Labels for the capability flags a model reports as true, CAP_LABELS order. */
export function capabilityLabels(capabilities: unknown): string[] {
  const flags = (capabilities ?? {}) as Record<string, unknown>;
  return Object.entries(CAP_LABELS).filter(([k]) => flags[k]).map(([, label]) => label);
}

type AnySpec = { kind?: string; [k: string]: unknown };

/** Compact one-line summary of a param spec (kind + allowed/range + default). */
export function describeSpec(spec: AnySpec): string {
  const kind = spec.kind ?? 'value';
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

/**
 * A request body that would pass this model's validation — what "Copy as curl"
 * should hand someone. A required image role gets placeholder URLs (an
 * image-only model rejects a prompt-only request), and a model that never
 * reads the prompt is not shown one.
 */
export function exampleBody(model: ModelInfo, schema: ModelSchema | null | undefined): Record<string, unknown> {
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
  return body;
}

/** The family endpoint a model's generations go to. */
export function generationEndpoint(model: ModelInfo): string {
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
