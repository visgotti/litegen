import { describe, expect, it } from 'vitest';
import type { ParamSpec } from '@litegen/sdk';
import {
  buildRequestForModel,
  clampToSpec,
  defaultForSpec,
  mergeParams,
  paramDescription,
  paramLabel,
} from './params';

// `clampToSpec` takes the loose internal spec shape, not the generated union.
type AnySpecForTest = Parameters<typeof clampToSpec>[0];

const spec = (extra: Record<string, unknown>): ParamSpec =>
  ({ kind: 'int', min: 100, max: 300000, ...extra }) as unknown as ParamSpec;

describe('paramLabel', () => {
  it('uses the declared label when present', () => {
    expect(paramLabel('target_polycount', spec({ label: 'Triangle budget' }))).toBe('Triangle budget');
  });

  it('humanises the key when the label is absent', () => {
    expect(paramLabel('target_polycount', spec({}))).toBe('Target polycount');
  });

  it('humanises the key when the label is null', () => {
    expect(paramLabel('target_polycount', spec({ label: null }))).toBe('Target polycount');
  });

  it('treats an empty or whitespace-only label as absent', () => {
    expect(paramLabel('guidance_scale', spec({ label: '' }))).toBe('Guidance scale');
    expect(paramLabel('guidance_scale', spec({ label: '   ' }))).toBe('Guidance scale');
  });

  it('trims surrounding whitespace from a declared label', () => {
    expect(paramLabel('steps', spec({ label: '  Steps  ' }))).toBe('Steps');
  });

  it('humanises a multi-part snake_case key, lowercasing all but the first word', () => {
    expect(paramLabel('negative_prompt_strength', spec({}))).toBe('Negative prompt strength');
  });

  it('capitalises a single-word key', () => {
    expect(paramLabel('topology', spec({}))).toBe('Topology');
  });

  it('ignores a non-string label from an unvalidated payload', () => {
    expect(paramLabel('symmetry', spec({ label: 42 }))).toBe('Symmetry');
  });
});

describe('paramDescription', () => {
  it('returns the trimmed description when present', () => {
    expect(paramDescription(spec({ description: ' Approximate triangle budget. ' })))
      .toBe('Approximate triangle budget.');
  });

  it('returns null when the description is absent, null, empty or not a string', () => {
    expect(paramDescription(spec({}))).toBeNull();
    expect(paramDescription(spec({ description: null }))).toBeNull();
    expect(paramDescription(spec({ description: '  ' }))).toBeNull();
    expect(paramDescription(spec({ description: 7 }))).toBeNull();
  });
});

// ─── string_array (output_formats) ──────────────────────────────────────────
//
// Every member selected here is a container the generation MUST deliver or
// fail, so the cost of getting these wrong is a rejected — or worse, a failed
// and billed — run rather than a cosmetic glitch.

const formats = (extra: Record<string, unknown>): ParamSpec =>
  ({ kind: 'string_array', enum_values: ['glb', 'obj', 'stl'], ...extra }) as unknown as ParamSpec;

describe('defaultForSpec for string_array', () => {
  it('uses the declared default array', () => {
    expect(defaultForSpec(formats({ default: ['glb', 'obj'] }))).toEqual(['glb', 'obj']);
  });

  it('falls back to the first format only, never to all of them', () => {
    // Each extra container is a file the caller pays for and a format the
    // generation can now fail on; nobody should be opted into both by default.
    expect(defaultForSpec(formats({}))).toEqual(['glb']);
    expect(defaultForSpec(formats({ default: [] }))).toEqual(['glb']);
  });

  it('is an empty array when the model declares no formats', () => {
    expect(defaultForSpec(formats({ enum_values: [] }))).toEqual([]);
  });
});

describe('clampToSpec for string_array', () => {
  it('drops formats the model does not declare', () => {
    // Compare mode shares one selection across models. Without narrowing, one
    // model offering stl would fail every other model in the run.
    const s = { kind: 'string_array', enum_values: ['glb', 'obj'] } as unknown as AnySpecForTest;
    expect(clampToSpec(s, ['glb', 'stl', 'obj'])).toEqual(['glb', 'obj']);
  });

  it('truncates to the model max_items', () => {
    const s = { kind: 'string_array', enum_values: ['glb', 'obj', 'stl'], max_items: 1 } as unknown as AnySpecForTest;
    expect(clampToSpec(s, ['glb', 'obj', 'stl'])).toEqual(['glb']);
  });

  it('can narrow to nothing when the sets do not overlap', () => {
    const s = { kind: 'string_array', enum_values: ['glb'] } as unknown as AnySpecForTest;
    expect(clampToSpec(s, ['stl'])).toEqual([]);
  });
});

describe('buildRequestForModel with string_array', () => {
  const schema = (params: Record<string, unknown>) =>
    ({ params }) as unknown as Parameters<typeof buildRequestForModel>[1];
  const shared = (params: Record<string, unknown>) =>
    ({ prompt: 'a fox', n: 1, strict: false, seed: '', params }) as unknown as Parameters<typeof buildRequestForModel>[2];

  it('sends the narrowed selection', () => {
    const body = buildRequestForModel(
      'mock/all-params-3d',
      schema({ output_formats: { kind: 'string_array', enum_values: ['glb', 'obj'] } }),
      shared({ output_formats: ['glb', 'obj', 'stl'] }),
    ) as unknown as Record<string, unknown>;
    expect(body.output_formats).toEqual(['glb', 'obj']);
  });

  it('omits the field entirely when narrowing empties it', () => {
    // An explicitly empty list is rejected by the API with param_out_of_range;
    // omitting it is what makes the model's own default apply.
    const body = buildRequestForModel(
      'mock/mesh-3d',
      schema({ output_formats: { kind: 'string_array', enum_values: ['glb'] } }),
      shared({ output_formats: ['stl'] }),
    ) as unknown as Record<string, unknown>;
    expect('output_formats' in body).toBe(false);
  });

  it('drops the param for a model that does not declare it', () => {
    const body = buildRequestForModel(
      'mock/fail-3d',
      schema({}),
      shared({ output_formats: ['glb'] }),
    ) as unknown as Record<string, unknown>;
    expect('output_formats' in body).toBe(false);
  });
});

describe('mergeParams for string_array', () => {
  it('intersects the formats and takes the smallest cap', () => {
    const merged = mergeParams({
      'a/one': { params: { output_formats: { kind: 'string_array', enum_values: ['glb', 'obj', 'stl'], max_items: 3, default: ['glb', 'obj'] } } },
      'b/two': { params: { output_formats: { kind: 'string_array', enum_values: ['glb', 'obj'], max_items: 1 } } },
    } as unknown as Parameters<typeof mergeParams>[0]);
    const spec = merged.find(p => p.name === 'output_formats')!.spec as unknown as Record<string, unknown>;
    expect(spec.enum_values).toEqual(['glb', 'obj']);
    expect(spec.max_items).toBe(1);
    // The carried-over default must also fit inside the intersected set.
    expect(spec.default).toEqual(['glb', 'obj']);
  });

  it('drops a default naming a format the intersection removed', () => {
    const merged = mergeParams({
      'a/one': { params: { output_formats: { kind: 'string_array', enum_values: ['glb', 'stl'], default: ['stl'] } } },
      'b/two': { params: { output_formats: { kind: 'string_array', enum_values: ['glb'] } } },
    } as unknown as Parameters<typeof mergeParams>[0]);
    const spec = merged.find(p => p.name === 'output_formats')!.spec as unknown as Record<string, unknown>;
    expect(spec.enum_values).toEqual(['glb']);
    expect(spec.default).toEqual([]);
  });
});
