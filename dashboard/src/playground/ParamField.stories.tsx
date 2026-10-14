import { useState, type ComponentProps } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ParamSpec } from '@litegen/sdk';
import { fn } from 'storybook/test';
import ParamField from './ParamField';
import { defaultForSpec } from './params';

type Props = ComponentProps<typeof ParamField>;

/** ParamField is controlled; this holds the value so the control is live. */
function Controlled({ value, onChange, ...rest }: Props) {
  const [v, setV] = useState<unknown>(value);
  return <ParamField {...rest} value={v} onChange={next => { setV(next); onChange(next); }} />;
}

const MODELS = ['mock/all-params-3d', 'mock/mesh-3d'];

/** Story args for one field: applies to every selected model, default value. */
function field(name: string, spec: ParamSpec, models: string[] = MODELS): Props {
  return { name, spec, models, totalSelected: MODELS.length, value: defaultForSpec(spec), onChange: fn() };
}

/**
 * One row of the Compare-mode unified parameter panel. The label is the
 * schema's `label` (else the key humanised) and a declared `description`
 * shows as a help line and tooltip; the right-hand note lists which selected
 * models the param applies to.
 */
const meta = {
  title: 'Playground/ParamField',
  component: ParamField,
  render: args => <Controlled {...args} />,
  decorators: [Story => <div className="pg-params" style={{ maxWidth: 360 }}><Story /></div>],
  args: field('steps', { kind: 'int', min: 1, max: 50, default: 28 }),
} satisfies Meta<typeof ParamField>;
export default meta;
type Story = StoryObj<typeof meta>;

// ─── Every ParamSpec kind ───────────────────────────────────────────────────

export const Bool: Story = { args: field('texture', { kind: 'bool', default: true }) };
export const Int: Story = { args: field('steps', { kind: 'int', min: 1, max: 50, default: 28 }) };
export const Float: Story = {
  args: field('guidance_scale', { kind: 'float', min: 0, max: 20, default: 7.5 }),
};
export const StringFreeText: Story = {
  args: field('negative_prompt', { kind: 'string', max_length: 500 }),
};
export const StringEnum: Story = {
  args: field('style', { kind: 'string', enum_values: ['vivid', 'natural'], default: 'vivid' }),
};
export const Seed: Story = { args: field('seed', { kind: 'seed', min: 0, max: 2147483647 }) };
export const SizeEnum: Story = {
  args: field('size', { kind: 'size', mode: 'enum', values: [[1024, 1024], [1536, 1024], [1024, 1536]] }),
};
export const SizeFreeform: Story = {
  args: field('size', {
    kind: 'size', mode: 'freeform', min_width: 256, max_width: 2048, min_height: 256, max_height: 2048, multiple_of: 64,
  }),
};
export const AspectRatio: Story = {
  args: field('aspect_ratio', { kind: 'aspect_ratio', allowed: ['1:1', '16:9', '9:16', '4:3'], default: '1:1' }),
};

// ─── Label / description ────────────────────────────────────────────────────

/** No `label` or `description`: the key is humanised and there is no help line. */
export const WithoutLabelOrDescription: Story = {
  args: field('target_polycount', { kind: 'int', min: 100, max: 300000 }),
};

/** Both declared: the label replaces the key, the description shows beneath
 *  the control and as the label's tooltip. */
export const WithLabelAndDescription: Story = {
  args: field('target_polycount', {
    kind: 'int', min: 100, max: 300000, label: 'Target polycount', description: 'Approximate triangle budget.',
  }),
};

/** The same param without and with schema text, one above the other. */
export const LabelFallbackSideBySide: Story = {
  render: () => (
    <>
      <Controlled {...field('guidance_scale', { kind: 'float', min: 0, max: 20, default: 7.5 })} />
      <Controlled
        {...field('guidance_scale', {
          kind: 'float', min: 0, max: 20, default: 7.5,
          label: 'Prompt adherence', description: 'Higher follows the prompt more literally; lower gives the model more freedom.',
        })}
      />
    </>
  ),
};

/** Declared by only some of the selected models: the note names them. */
export const PartialApplicability: Story = {
  args: field('rig', { kind: 'bool', default: false, label: 'Auto-rig' }, ['mock/all-params-3d']),
};

// ─── The 3D params (shaped like GET /v1/models/{id}) ─────────────────────────

/** `output_formats` as served: `enum_values` is everything a caller may ask
 *  for, `max_items` counts provider jobs, and `derivable_values` lists the
 *  formats litegen derives from one source — any subset of which is ONE job. */
function formats(extra: Record<string, unknown>): ParamSpec {
  return { kind: 'string_array', default: ['glb'], label: 'Output formats', ...extra } as unknown as ParamSpec;
}
/** Every container litegen converts — the shape every 3D model serves today:
 *  one vendor job yields all ten, so the cap cannot bind. */
const ALL_TEN = ['glb', 'gltf', 'obj', 'stl', 'ply', '3mf', 'dae', 'fbx', 'usdz', 'usda'];
const EVERY_FORMAT_ONE_JOB = formats({
  enum_values: ALL_TEN,
  derivable_values: ALL_TEN,
  max_items: 1,
  description: 'TRELLIS emits glb; litegen converts it to every other container.',
});
/** A model offering containers litegen cannot convert, each a provider job of
 *  its own (Rodin's shape before litegen learned fbx and usdz). Nothing serves
 *  this today; it is the case the budget rule exists for. */
const PARTLY_DERIVABLE_FORMATS = formats({
  enum_values: ['glb', 'usdz', 'fbx', 'obj', 'stl', 'ply'],
  derivable_values: ['glb', 'obj', 'stl', 'ply'],
  max_items: 1,
});
const TOPOLOGY: ParamSpec = { kind: 'string', enum_values: ['triangle', 'quad'], default: 'triangle', label: 'Topology' };
const SYMMETRY: ParamSpec = { kind: 'string', enum_values: ['off', 'auto', 'on'], default: 'auto', label: 'Symmetry' };
const TARGET_POLYCOUNT: ParamSpec = {
  kind: 'int', min: 100, max: 300000, label: 'Target polycount', description: 'Approximate triangle budget.',
};

/** Today's case: every format comes out of the one provider job, so any
 *  combination is allowed and the hint says so. */
export const OutputFormatsAnyCombination: Story = {
  args: { ...field('output_formats', EVERY_FORMAT_ONE_JOB, ['fal/trellis']), value: ['glb', 'fbx', 'usdz'] },
};

/** A container litegen cannot convert is a provider job of its own: with any
 *  shared format ticked, the vendor-only ones are disabled (and vice versa). */
export const OutputFormatsOneJob: Story = {
  args: field('output_formats', PARTLY_DERIVABLE_FORMATS),
};

/** Every derivable container ticked — still one provider job. */
export const OutputFormatsWholeDerivableSet: Story = {
  args: { ...field('output_formats', PARTLY_DERIVABLE_FORMATS), value: ['glb', 'obj', 'stl', 'ply'] },
};

/** A server that predates `derivable_values`: every format counts as its own
 *  job, so `max_items: 1` means exactly one box. */
export const OutputFormatsWithoutDerivableValues: Story = {
  args: field('output_formats', formats({ enum_values: ['glb', 'obj', 'stl'], max_items: 1 })),
};

export const Topology: Story = { args: field('topology', TOPOLOGY) };
export const Symmetry: Story = { args: field('symmetry', SYMMETRY) };
export const TargetPolycount: Story = { args: field('target_polycount', TARGET_POLYCOUNT) };

// ─── Unset vs set ───────────────────────────────────────────────────────────
//
// An untouched param is never sent, so each model applies its own default; a
// control with no declared default shows that rather than inventing a value.

/** No declared default: an empty input whose placeholder says the model decides. */
export const UnsetNumber: Story = { args: field('target_polycount', TARGET_POLYCOUNT) };

/** No declared default: the select leads with "Model default", which sends nothing. */
export const EnumWithoutDefault: Story = {
  args: field('quality', { kind: 'string', enum_values: ['low', 'medium', 'high'] }),
};

/** Set by the user: "reset" clears it back to the model's default. */
export const SetWithReset: Story = {
  args: { ...field('target_polycount', TARGET_POLYCOUNT), value: 20000, onReset: fn() },
};

/** The whole panel for `mock/all-params-3d` + `mock/mesh-3d` selected together. */
export const All3DParamsPanel: Story = {
  render: () => (
    <>
      <Controlled {...field('output_formats', EVERY_FORMAT_ONE_JOB)} />
      <Controlled {...field('texture', { kind: 'bool', default: true, label: 'Textures' })} />
      <Controlled {...field('pbr', { kind: 'bool', default: false, label: 'PBR materials' }, ['mock/all-params-3d'])} />
      <Controlled {...field('rig', { kind: 'bool', default: false, label: 'Auto-rig' }, ['mock/all-params-3d'])} />
      <Controlled {...field('symmetry', SYMMETRY, ['mock/all-params-3d'])} />
      <Controlled {...field('target_polycount', TARGET_POLYCOUNT)} />
      <Controlled {...field('topology', TOPOLOGY, ['mock/all-params-3d'])} />
    </>
  ),
};
