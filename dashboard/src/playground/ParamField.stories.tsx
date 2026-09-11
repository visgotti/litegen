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

// ─── The 3D params (as models/mock.yaml declares them) ─────────────────────

const OUTPUT_FORMAT: ParamSpec = {
  kind: 'string', enum_values: ['glb', 'obj', 'fbx', 'usdz'], default: 'glb',
  label: 'Output format', description: 'Mesh container. Only GLB/glTF previews in the browser.',
};
const TOPOLOGY: ParamSpec = { kind: 'string', enum_values: ['triangle', 'quad'], default: 'triangle', label: 'Topology' };
const SYMMETRY: ParamSpec = { kind: 'string', enum_values: ['off', 'auto', 'on'], default: 'auto', label: 'Symmetry' };
const TARGET_POLYCOUNT: ParamSpec = {
  kind: 'int', min: 100, max: 300000, label: 'Target polycount', description: 'Approximate triangle budget.',
};

export const Topology: Story = { args: field('topology', TOPOLOGY) };
export const Symmetry: Story = { args: field('symmetry', SYMMETRY) };
export const OutputFormat: Story = { args: field('output_format', OUTPUT_FORMAT) };
export const TargetPolycount: Story = { args: field('target_polycount', TARGET_POLYCOUNT) };

/** The whole panel for `mock/all-params-3d` + `mock/mesh-3d` selected together. */
export const All3DParamsPanel: Story = {
  render: () => (
    <>
      <Controlled {...field('output_format', OUTPUT_FORMAT)} />
      <Controlled {...field('texture', { kind: 'bool', default: true, label: 'Textures' })} />
      <Controlled {...field('pbr', { kind: 'bool', default: false, label: 'PBR materials' }, ['mock/all-params-3d'])} />
      <Controlled {...field('rig', { kind: 'bool', default: false, label: 'Auto-rig' }, ['mock/all-params-3d'])} />
      <Controlled {...field('symmetry', SYMMETRY, ['mock/all-params-3d'])} />
      <Controlled {...field('target_polycount', TARGET_POLYCOUNT)} />
      <Controlled {...field('topology', TOPOLOGY, ['mock/all-params-3d'])} />
    </>
  ),
};
