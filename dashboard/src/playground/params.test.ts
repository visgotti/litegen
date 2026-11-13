import { describe, expect, it } from 'vitest';
import type { ModelInfo, ParamSpec } from '@litegen/sdk';
import {
  allShareOneJob,
  buildRequestForModel,
  canAddToSet,
  clampToSpec,
  defaultForSpec,
  docsUrlOf,
  extraForModel,
  extraFromBody,
  extraToRaw,
  maxRequestBytesOf,
  maxTotalRefs,
  mergeExtras,
  mergeParams,
  mergeRefRoles,
  paramDescription,
  paramLabel,
  paramsFromBody,
  parseExtraValue,
  promptForModel,
  promptLength,
  promptMaxLength,
  promptUsage,
  refsForModel,
  refsFromBody,
  roleLabel,
  schemaInputs,
  vendorJobsNeeded,
  wireValue,
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

  it('shows the default the server serves, and invents none', () => {
    // GET /v1/models/{id} serves the list an omitted output_formats resolves
    // to (E11d), so a format set the page is handed without one gets nothing
    // ticked rather than a client's guess at the server's fallback.
    expect(defaultForSpec(formats({ default: ['obj'] }))).toEqual(['obj']);
    expect(defaultForSpec(formats({}))).toEqual([]);
    expect(defaultForSpec(formats({ default: [] }))).toEqual([]);
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

// ─── The provider-job budget (`derivable_values`) ──────────────────────────
//
// Fixtures for the rule, not the catalog. `enum_values` is everything a caller
// may ask for, `max_items` counts PROVIDER JOBS, and `derivable_values` lists the
// formats litegen derives locally from one source — any subset of which costs a
// single job. `trellis` is a model whose every format is derivable: the shape
// every 3D model serves since litegen converts between all its containers.
// `partlyDerivable` offers containers litegen cannot convert, each a provider
// job of its own (Rodin's shape before litegen learned fbx and usdz); it keeps
// the budget logic tested for the next model like it.

const partlyDerivable = {
  kind: 'string_array',
  enum_values: ['glb', 'usdz', 'fbx', 'obj', 'stl', 'ply'],
  derivable_values: ['glb', 'obj', 'stl', 'ply'],
  max_items: 1,
  default: ['glb'],
} as unknown as AnySpecForTest;

const trellis = {
  kind: 'string_array',
  enum_values: ['glb', 'obj', 'stl', 'ply'],
  derivable_values: ['glb', 'obj', 'stl', 'ply'],
  max_items: 1,
  default: ['glb'],
} as unknown as AnySpecForTest;

describe('vendorJobsNeeded', () => {
  it('counts the whole derivable set as one job', () => {
    expect(vendorJobsNeeded(['glb', 'obj', 'stl', 'ply'], ['glb', 'obj', 'stl', 'ply'])).toBe(1);
  });

  it('counts each non-derivable format as its own job', () => {
    expect(vendorJobsNeeded(['glb', 'fbx'], ['glb', 'obj', 'stl', 'ply'])).toBe(2);
    expect(vendorJobsNeeded(['fbx', 'usdz'], ['glb', 'obj', 'stl', 'ply'])).toBe(2);
    expect(vendorJobsNeeded(['fbx'], ['glb', 'obj', 'stl', 'ply'])).toBe(1);
  });

  it('counts every format separately when the server names none as derivable', () => {
    expect(vendorJobsNeeded(['glb', 'obj'], [])).toBe(2);
  });

  it('is zero for an empty selection', () => {
    expect(vendorJobsNeeded([], ['glb'])).toBe(0);
  });
});

describe('canAddToSet', () => {
  it('takes every derivable format from the one job', () => {
    expect(canAddToSet(partlyDerivable, ['glb'], 'obj')).toBe(true);
    expect(canAddToSet(partlyDerivable, ['glb', 'obj', 'stl'], 'ply')).toBe(true);
  });

  it('refuses a second provider job for a container litegen cannot convert', () => {
    expect(canAddToSet(partlyDerivable, ['glb'], 'fbx')).toBe(false);
    expect(canAddToSet(partlyDerivable, ['fbx'], 'glb')).toBe(false);
    expect(canAddToSet(partlyDerivable, ['fbx'], 'usdz')).toBe(false);
  });

  it('allows anything when the spec declares no cap', () => {
    const uncapped = { kind: 'string_array', enum_values: ['glb', 'fbx'] } as unknown as AnySpecForTest;
    expect(canAddToSet(uncapped, ['glb'], 'fbx')).toBe(true);
  });

  it('falls back to counting formats when derivable_values is absent (an older server)', () => {
    const legacy = { kind: 'string_array', enum_values: ['glb', 'obj'], max_items: 1 } as unknown as AnySpecForTest;
    expect(canAddToSet(legacy, ['glb'], 'obj')).toBe(false);
  });
});

describe('allShareOneJob', () => {
  it('is true when every format on offer is derivable — no selection can exceed the budget', () => {
    expect(allShareOneJob(trellis)).toBe(true);
    expect(allShareOneJob({ kind: 'string_array', enum_values: ['fbx', 'glb'], derivable_values: ['glb', 'gltf', 'fbx'] } as unknown as AnySpecForTest)).toBe(true);
  });

  it('is false when some format is a job of its own, or nothing is derivable', () => {
    expect(allShareOneJob(partlyDerivable)).toBe(false); // fbx/usdz in this fixture are not derivable
    expect(allShareOneJob({ kind: 'string_array', enum_values: ['glb'] } as unknown as AnySpecForTest)).toBe(false);
    expect(allShareOneJob({ kind: 'string_array', enum_values: [], derivable_values: ['glb'] } as unknown as AnySpecForTest)).toBe(false);
  });
});

describe('clampToSpec with derivable_values', () => {
  it('keeps a whole derivable set on a one-job model', () => {
    expect(clampToSpec(trellis, ['glb', 'obj', 'stl', 'ply'])).toEqual(['glb', 'obj', 'stl', 'ply']);
  });

  it('keeps formats in order while they fit the job budget', () => {
    expect(clampToSpec(partlyDerivable, ['glb', 'fbx', 'obj'])).toEqual(['glb', 'obj']);
    expect(clampToSpec(partlyDerivable, ['fbx', 'glb'])).toEqual(['fbx']);
  });

  it('drops a format the model cannot deliver at all', () => {
    expect(clampToSpec(trellis, ['glb', 'fbx'])).toEqual(['glb']);
  });
});

describe('mergeParams with derivable_values', () => {
  it('intersects the derivable sets', () => {
    const merged = mergeParams({
      'x/partly-derivable': { params: { output_formats: partlyDerivable } },
      'fal/trellis': { params: { output_formats: trellis } },
    } as unknown as Parameters<typeof mergeParams>[0]);
    const spec = merged.find(p => p.name === 'output_formats')!.spec as unknown as Record<string, unknown>;
    expect(spec.enum_values).toEqual(['glb', 'obj', 'stl', 'ply']);
    expect(spec.derivable_values).toEqual(['glb', 'obj', 'stl', 'ply']);
    expect(spec.max_items).toBe(1);
  });

  it('drops the derivable set when one model does not declare it', () => {
    const merged = mergeParams({
      'a/one': { params: { output_formats: trellis } },
      'b/two': { params: { output_formats: { kind: 'string_array', enum_values: ['glb', 'obj'], max_items: 2 } } },
    } as unknown as Parameters<typeof mergeParams>[0]);
    const spec = merged.find(p => p.name === 'output_formats')!.spec as unknown as Record<string, unknown>;
    expect('derivable_values' in spec).toBe(false);
  });

  it('lets a caller skip params it renders its own control for', () => {
    const merged = mergeParams(
      { 'a/one': { params: { size: { kind: 'size', mode: 'freeform' }, steps: { kind: 'int' } } } } as unknown as Parameters<typeof mergeParams>[0],
      new Set(['size']),
    );
    expect(merged.map(p => p.name)).toEqual(['steps']);
  });
});

// ─── Untouched params are never sent ────────────────────────────────────────

describe('defaultForSpec never invents a value to send', () => {
  it('shows an int or float without a declared default as unset, not its minimum', () => {
    // The old fallback sent target_polycount: 100 on every 3D Compare run.
    expect(defaultForSpec(spec({}))).toBe('');
    expect(defaultForSpec({ kind: 'float', min: 1, max: 20 } as unknown as ParamSpec)).toBe('');
  });

  it('shows the declared default when there is one', () => {
    expect(defaultForSpec(spec({ default: 20000 }))).toBe(20000);
    expect(defaultForSpec({ kind: 'bool', default: true } as unknown as ParamSpec)).toBe(true);
    expect(defaultForSpec({ kind: 'string', enum_values: ['triangle', 'quad'], default: 'quad' } as unknown as ParamSpec)).toBe('quad');
  });

  it('shows an enum without a declared default as unset, not its first member', () => {
    expect(defaultForSpec({ kind: 'string', enum_values: ['low', 'high'] } as unknown as ParamSpec)).toBe('');
    expect(defaultForSpec({ kind: 'aspect_ratio', allowed: ['1:1', '16:9'] } as unknown as ParamSpec)).toBe('');
    expect(defaultForSpec({ kind: 'size', mode: 'enum', values: [[512, 512]] } as unknown as ParamSpec)).toBe('');
  });

  it('shows a format set the default it is served with, and none without one', () => {
    expect(defaultForSpec(partlyDerivable as unknown as ParamSpec)).toEqual(['glb']);
    expect(defaultForSpec({ kind: 'string_array', enum_values: ['fbx', 'glb'] } as unknown as ParamSpec)).toEqual([]);
  });
});

const schemaOf = (s: Record<string, unknown>) => s as unknown as Parameters<typeof buildRequestForModel>[1];
const state = (s: Record<string, unknown>) =>
  ({ prompt: 'a fox', n: 1, strict: true, seed: '', params: {}, refs: [], extra: {}, ...s }) as unknown as Parameters<typeof buildRequestForModel>[2];

describe('buildRequestForModel', () => {
  const mesh = schemaOf({
    params: {
      target_polycount: { kind: 'int', min: 100, max: 300000 },
      texture: { kind: 'bool', default: true },
      output_formats: trellis,
    },
  });

  it('sends only what the user set', () => {
    const body = buildRequestForModel('mock/mesh-3d', mesh, state({})) as unknown as Record<string, unknown>;
    expect(body).toEqual({ model: 'mock/mesh-3d', prompt: 'a fox', strict: true });
  });

  it('sends a set value, converting a typed number', () => {
    const body = buildRequestForModel('mock/mesh-3d', mesh,
      state({ params: { target_polycount: '20000', texture: false } })) as unknown as Record<string, unknown>;
    expect(body.target_polycount).toBe(20000);
    expect(body.texture).toBe(false);
  });

  it('drops a typed number that does not parse', () => {
    const body = buildRequestForModel('mock/mesh-3d', mesh,
      state({ params: { target_polycount: 'abc' } })) as unknown as Record<string, unknown>;
    expect('target_polycount' in body).toBe(false);
  });

  it('sends n for images only — one 3D job is one result', () => {
    // One image per request (the validator refuses n > 1): N images are N
    // requests, and one 3D job is one result.
    expect(buildRequestForModel('m', schemaOf({}), state({ n: 3 }))).not.toHaveProperty('n');
  });

  it('sends seed only to a model that declares it', () => {
    expect(buildRequestForModel('m', schemaOf({ params: { seed: { kind: 'seed' } } }), state({ seed: '7' }))).toHaveProperty('seed', 7);
    expect(buildRequestForModel('m', schemaOf({}), state({ seed: '7' }))).not.toHaveProperty('seed');
  });
});

// ─── Listed and bounded numbers: Compare narrows, the server judges ─────────
//
// Compare mode shares one value across models, so a model that lists its
// values is sent only a value it lists (E11e). Otherwise a number goes as
// typed, in its bounds or not: the server's check says what it takes (N5).

describe('a listed value in a Compare run (E11e)', () => {
  const listed = { kind: 'int', values: [512, 1024, 2048] } as unknown as AnySpecForTest;
  const textureModel = (values: number[]) => schemaOf({ params: { texture_resolution: { kind: 'int', values } } });

  it('sends a model that takes listed values only a listed one', () => {
    expect(clampToSpec(listed, 1024, { compare: true })).toBe(1024);
    expect(clampToSpec(listed, 768, { compare: true })).toBeUndefined();
  });

  it('leaves a listed param off a Compare model that does not list the shared value', () => {
    const body = buildRequestForModel('mock/texture-3d', textureModel([1024, 2048]),
      state({ params: { texture_resolution: '512' } })) as unknown as Record<string, unknown>;
    expect('texture_resolution' in body).toBe(false);
  });

  it('offers in Compare only the listed values every model takes', () => {
    const merged = mergeParams({ a: textureModel([512, 1024, 2048]), b: textureModel([1024, 2048, 4096]) });
    expect((merged[0].spec as unknown as { values: number[] }).values).toEqual([1024, 2048]);
  });

  it('keeps the one list when only one of the models lists values', () => {
    const ranged = schemaOf({ params: { texture_resolution: { kind: 'int', min: 256, max: 4096 } } });
    const merged = mergeParams({ a: ranged, b: textureModel([512, 1024]) });
    expect((merged[0].spec as unknown as { values: number[] }).values).toEqual([512, 1024]);
  });

  it('sends Single mode the value picked, listed or not: the server says why it will not do', () => {
    // A history entry restored after the catalog changed is the one way there.
    expect(schemaInputs(textureModel([1024, 2048]), { params: { texture_resolution: '512' } }))
      .toEqual({ texture_resolution: 512 });
  });
});

describe('a typed number goes as typed (N5)', () => {
  const steps = schemaOf({ params: { steps: { kind: 'int', min: 1, max: 50 } } });

  it('is not moved into its bounds: the server answers param_out_of_range, in its words', () => {
    expect(schemaInputs(steps, { params: { steps: '500' } })).toEqual({ steps: 500 });
    expect(buildRequestForModel('m', steps, state({ params: { steps: '0' } }))).toHaveProperty('steps', 0);
  });
});

// ─── Reference images ───────────────────────────────────────────────────────

const trellisSchema = schemaOf({
  // As served: `max_length: 0` is the server's "takes no prompt at all".
  prompt: { required: false, max_length: 0 },
  capabilities: { image_to_3d: true },
  ref_inputs: { max_total: 1, default_role: 'init', roles: { init: { required: true, min_count: 1, max_count: 1 } } },
  params: { output_formats: trellis },
  extra_allowlist: ['texture_size', 'mesh_simplify'],
});

const rodinSchema = schemaOf({
  prompt: { required: false, max_length: 1024 },
  capabilities: { text_to_3d: true, image_to_3d: true },
  ref_inputs: { max_total: 5, default_role: 'init', roles: { init: { required: false, min_count: 0, max_count: 5 } } },
  extra_allowlist: ['quality', 'tier'],
});

const multiviewSchema = schemaOf({
  prompt: { required: true },
  capabilities: { text_to_3d: true, image_to_3d: true, multiview_to_3d: true },
  ref_inputs: {
    max_total: 6,
    roles: {
      init: { required: false, min_count: 0, max_count: 1 },
      'view-front': { required: false, min_count: 0, max_count: 1 },
      'view-back': { required: false, min_count: 0, max_count: 1 },
    },
  },
});

const ref = (role: string, value = `https://img.test/${role}.png`) =>
  ({ id: `${role}-${value}`, role, type: 'url' as const, value });

describe('refsForModel', () => {
  it('sends only the roles a model declares', () => {
    expect(refsForModel(trellisSchema, [ref('init'), ref('view-front')])).toEqual([
      { type: 'url', value: 'https://img.test/init.png', role: 'init' },
    ]);
  });

  it('caps each role at the model own max_count', () => {
    const two = [ref('init', 'https://img.test/a.png'), ref('init', 'https://img.test/b.png')];
    expect(refsForModel(trellisSchema, two)).toHaveLength(1);
    expect(refsForModel(rodinSchema, two)).toHaveLength(2);
  });

  it('caps the total at max_total', () => {
    const many = ['init', 'view-front', 'view-back'].map(r => ref(r));
    const capped = refsForModel(schemaOf({ ref_inputs: { max_total: 2, roles: { init: { max_count: 1 }, 'view-front': { max_count: 1 }, 'view-back': { max_count: 1 } } } }), many);
    expect(capped.map(r => r.role)).toEqual(['init', 'view-front']);
  });

  it('sends nothing to a model without reference inputs', () => {
    expect(refsForModel(schemaOf({}), [ref('init')])).toEqual([]);
  });

  it('carries an upload inline as base64', () => {
    const upload = { id: 'u', role: 'init', type: 'base64' as const, value: 'data:image/png;base64,iVBOR', name: 'chair.png' };
    expect(refsForModel(trellisSchema, [upload])).toEqual([{ type: 'base64', value: 'data:image/png;base64,iVBOR', role: 'init' }]);
  });

  it('puts the refs on the request built for the model', () => {
    const body = buildRequestForModel('fal/trellis', trellisSchema, state({ prompt: '', refs: [ref('init')] })) as unknown as Record<string, unknown>;
    expect(body.reference_images).toEqual([{ type: 'url', value: 'https://img.test/init.png', role: 'init' }]);
  });
});

describe('mergeRefRoles', () => {
  it('unions roles, taking the strictest cap and recording who requires each', () => {
    const roles = mergeRefRoles({ 'fal/trellis': trellisSchema, 'fal/hyper3d-rodin': rodinSchema, 'mock/all-params-3d': multiviewSchema });
    expect(roles.map(r => r.role)).toEqual(['init', 'view-front', 'view-back']);
    const init = roles[0];
    expect(init.maxCount).toBe(1);
    expect(init.minCount).toBe(1);
    expect(init.requiredBy).toEqual(['fal/trellis']);
    expect(init.models).toEqual(['fal/trellis', 'fal/hyper3d-rodin', 'mock/all-params-3d']);
    expect(roles[1].requiredBy).toEqual([]);
  });

  it('is empty when no selected model takes images', () => {
    expect(mergeRefRoles({ 'mock/fail-3d': schemaOf({}) })).toEqual([]);
  });

  it('marks a required role as needing an image even when its min_count is 0', () => {
    // The server refuses a `required` role sent no image, whatever min_count says.
    const roles = mergeRefRoles({ 'x/m': schemaOf({ ref_inputs: { roles: { init: { required: true, min_count: 0, max_count: 1 } } } }) });
    expect(roles[0].requiredBy).toEqual(['x/m']);
    expect(roles[0].minCount).toBe(1);
  });
});

describe('maxRequestBytesOf', () => {
  const info = (id: string, capabilities: Record<string, unknown>) => ({ id, capabilities }) as unknown as ModelInfo;
  it("is the smallest request limit the selected models' listing states, or undefined", () => {
    const models = [info('a', { max_request_bytes: 26_214_400 }), info('b', { max_request_bytes: 1_000_000 }), info('c', {})];
    expect(maxRequestBytesOf(models, ['a'])).toBe(26_214_400);
    expect(maxRequestBytesOf(models, ['a', 'b'])).toBe(1_000_000);
    expect(maxRequestBytesOf(models, ['c'])).toBeUndefined();
    expect(maxRequestBytesOf(models, [])).toBeUndefined();
  });
});

describe('maxTotalRefs', () => {
  it('is the tightest max_total, or undefined when none is declared', () => {
    expect(maxTotalRefs({ a: trellisSchema, b: rodinSchema })).toBe(1);
    expect(maxTotalRefs({ a: schemaOf({}) })).toBeUndefined();
  });
});

describe('roleLabel', () => {
  it('names the roles in words', () => {
    expect(roleLabel('init')).toBe('Input image');
    expect(roleLabel('view-front')).toBe('Front view');
    expect(roleLabel('mask')).toBe('Mask');
  });
});

describe('promptUsage', () => {
  it('reads the prompt rule the server serves', () => {
    expect(promptUsage(multiviewSchema)).toBe('required');
    expect(promptUsage(rodinSchema)).toBe('optional');
    expect(promptUsage(trellisSchema)).toBe('unused');
    expect(promptUsage(undefined)).toBe('required');
  });

  it('calls a prompt unused only when max_length is 0, never from capability flags', () => {
    // An image-edit model with no text mode still reads its prompt.
    const editOnly = schemaOf({
      prompt: { required: false, max_length: 2000 },
      capabilities: { image_to_image: true },
      ref_inputs: { max_total: 1, roles: { init: { required: true, min_count: 1, max_count: 1 } } },
    });
    expect(promptUsage(editOnly)).toBe('optional');
  });
});

describe('prompt limits', () => {
  it('reads the limit, treating 0 as "no prompt" rather than a limit', () => {
    expect(promptMaxLength(rodinSchema)).toBe(1024);
    expect(promptMaxLength(trellisSchema)).toBeNull();
    expect(promptMaxLength(schemaOf({}))).toBeNull();
  });

  it('counts characters as the server does, not UTF-16 code units', () => {
    expect(promptLength('fox')).toBe(3);
    expect(promptLength('🦊🦊')).toBe(2);
  });
});

describe('promptForModel', () => {
  it('sends the prompt only to a model that takes one, and only when it is not blank', () => {
    expect(promptForModel(rodinSchema, 'a fox')).toBe('a fox');
    expect(promptForModel(rodinSchema, '   ')).toBeUndefined();
    expect(promptForModel(trellisSchema, 'a fox')).toBeUndefined();
  });

  it('keeps a mixed Compare run from sending TRELLIS the shared prompt', () => {
    // Strict mode answers a prompt on a no-prompt model with a 400.
    const shared = state({ prompt: 'a fox', refs: [ref('init')] });
    const trellisBody = buildRequestForModel('fal/trellis', trellisSchema, shared) as unknown as Record<string, unknown>;
    const rodinBody = buildRequestForModel('fal/hyper3d-rodin', rodinSchema, shared) as unknown as Record<string, unknown>;
    expect('prompt' in trellisBody).toBe(false);
    expect(rodinBody.prompt).toBe('a fox');
  });
});

// ─── Provider passthrough (`extra`) ─────────────────────────────────────────

describe('parseExtraValue / extraToRaw', () => {
  it('parses JSON where it can and keeps text otherwise', () => {
    expect(parseExtraValue('512')).toBe(512);
    expect(parseExtraValue('true')).toBe(true);
    expect(parseExtraValue('{"a":1}')).toEqual({ a: 1 });
    expect(parseExtraValue('high')).toBe('high');
    expect(parseExtraValue('"512"')).toBe('512');
    expect(parseExtraValue('   ')).toBeUndefined();
  });

  it('round-trips every value type through the text field', () => {
    for (const v of [512, true, null, 'high', '512', { a: [1, 2] }, ['x']]) {
      expect(parseExtraValue(extraToRaw(v))).toEqual(v);
    }
  });
});

describe('mergeExtras / extraForModel', () => {
  it('unions the allowlists with applicability', () => {
    expect(mergeExtras({ 'fal/trellis': trellisSchema, 'fal/hyper3d-rodin': rodinSchema })).toEqual([
      { key: 'mesh_simplify', models: ['fal/trellis'] },
      { key: 'quality', models: ['fal/hyper3d-rodin'] },
      { key: 'texture_size', models: ['fal/trellis'] },
      { key: 'tier', models: ['fal/hyper3d-rodin'] },
    ]);
  });

  it('sends each model only its own allowlisted keys, parsed, blanks omitted', () => {
    const extra = { texture_size: '1024', quality: 'high', mesh_simplify: '' };
    expect(extraForModel(trellisSchema, extra)).toEqual({ texture_size: 1024 });
    expect(extraForModel(rodinSchema, extra)).toEqual({ quality: 'high' });
  });

  it('leaves extra off the request when nothing applies', () => {
    expect(extraForModel(schemaOf({}), { quality: 'high' })).toBeUndefined();
    const body = buildRequestForModel('m', schemaOf({}), state({ extra: { quality: 'high' } })) as unknown as Record<string, unknown>;
    expect('extra' in body).toBe(false);
  });
});

// A vendor field the catalog types (`extra_params`, served by GET
// /v1/models/{id}) is checked like a param: a wrong type is refused in either
// mode (`param_invalid`). So it goes as its type, never as whatever its text
// happens to parse to.
describe('typed passthrough fields (extra_params)', () => {
  // fal FLUX.1 [pro] as served: its safety_tolerance is the string "1".."6".
  const fluxPro = schemaOf({
    extra_allowlist: ['safety_tolerance'],
    extra_params: { safety_tolerance: { kind: 'string', enum_values: ['1', '2', '3', '4', '5', '6'], default: '2' } },
  });
  const ultra = schemaOf({
    extra_allowlist: ['raw', 'image_prompt_strength', 'safety_tolerance', 'style_codes', 'note'],
    extra_params: {
      raw: { kind: 'bool' },
      image_prompt_strength: { kind: 'float', min: 0, max: 1, default: 0.1 },
      safety_tolerance: { kind: 'string', enum_values: [] },
      style_codes: { kind: 'string_array' },
    },
  });

  it("sends fal FLUX's safety_tolerance as the string it is typed as", () => {
    expect(extraForModel(fluxPro, { safety_tolerance: '2' })).toEqual({ safety_tolerance: '2' });
  });

  it('sends a flag, a number, free text and a list as their types, and an untyped field as JSON where it parses', () => {
    expect(extraForModel(ultra, { raw: 'true', image_prompt_strength: '0.25', safety_tolerance: '5', style_codes: 'AB12, CD34', note: '{"a":1}' }))
      .toEqual({ raw: true, image_prompt_strength: 0.25, safety_tolerance: '5', style_codes: ['AB12', 'CD34'], note: { a: 1 } });
  });

  it('restores a typed string from history as the same string', () => {
    expect(extraForModel(fluxPro, extraFromBody({ extra: { safety_tolerance: '2' } }))).toEqual({ safety_tolerance: '2' });
  });

  it('carries each key with the type the selected models give it', () => {
    expect(mergeExtras({ 'fal/flux-pro': fluxPro }))
      .toEqual([{ key: 'safety_tolerance', models: ['fal/flux-pro'], spec: { kind: 'string', enum_values: ['1', '2', '3', '4', '5', '6'], default: '2' } }]);
  });
});

// ─── Restoring history; docs links ──────────────────────────────────────────

describe('restoring a stored request', () => {
  const stored = {
    model: 'fal/hyper3d-rodin',
    prompt: 'a fox',
    strict: false,
    seed: 7,
    output_formats: ['glb', 'obj'],
    pbr: false,
    reference_images: [{ type: 'url', value: 'https://img.test/a.png', role: 'init' }],
    extra: { quality: 'high', tier: '512' },
  };

  it('pulls the schema params out, leaving base fields to their own controls', () => {
    expect(paramsFromBody(stored)).toEqual({ output_formats: ['glb', 'obj'], pbr: false });
  });

  it('restores URL reference images', () => {
    expect(refsFromBody(stored)).toEqual([{ id: 'restored-0', role: 'init', type: 'url', value: 'https://img.test/a.png' }]);
    expect(refsFromBody({ reference_images: [{ type: 'base64', value: 'x', role: 'init' }] })).toEqual([]);
  });

  it('restores extra as text that parses back to the same values', () => {
    const raw = extraFromBody(stored);
    expect(raw).toEqual({ quality: 'high', tier: '"512"' });
    expect(extraForModel(rodinSchema, raw)).toEqual({ quality: 'high', tier: '512' });
  });
});

describe('docsUrlOf', () => {
  it('returns an https docs_url', () => {
    expect(docsUrlOf(schemaOf({ docs_url: 'https://fal.ai/models/fal-ai/trellis/api' }))).toBe('https://fal.ai/models/fal-ai/trellis/api');
  });

  it('ignores a missing, non-https or malformed one', () => {
    expect(docsUrlOf(schemaOf({}))).toBeNull();
    expect(docsUrlOf(schemaOf({ docs_url: 'http://insecure.test' }))).toBeNull();
    expect(docsUrlOf(schemaOf({ docs_url: 'javascript:alert(1)' }))).toBeNull();
    expect(docsUrlOf(schemaOf({ docs_url: 'not a url' }))).toBeNull();
    expect(docsUrlOf(undefined)).toBeNull();
  });
});


describe('restoring a stored request keeps its structured values', () => {
  it('turns stored layout boxes back into the drafts the layout control edits', () => {
    const layout = [
      { kind: 'text', text: 'SALE', box: { x: 0.1, y: 0.2, w: 0.5, h: 0.1 } },
      { kind: 'object', prompt: 'a cat', box: { x: 0.5, y: 0.5, w: 0.25, h: 0.25 } },
    ];
    const restored = paramsFromBody({ prompt: 'p', layout });
    expect(restored.layout).toEqual([
      { kind: 'text', text: 'SALE', x: 0.1, y: 0.2, w: 0.5, h: 0.1 },
      { kind: 'object', prompt: 'a cat', x: 0.5, y: 0.5, w: 0.25, h: 0.25 },
    ]);
    // ...which send exactly what was stored.
    expect(wireValue({ kind: 'layout' } as unknown as ParamSpec, restored.layout)).toEqual(layout);
  });

  it("keeps a keyframe's time", () => {
    const stored = { reference_images: [{ type: 'url', role: 'keyframe', value: 'https://img.test/k.png', time_s: 1.5 }] };
    expect(refsFromBody(stored)).toEqual([
      { id: 'restored-0', role: 'keyframe', type: 'url', value: 'https://img.test/k.png', time_s: '1.5' },
    ]);
  });
});
