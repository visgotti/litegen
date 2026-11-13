import { useState, type ComponentProps } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ModelSchema } from '@litegen/sdk';
import { fn } from 'storybook/test';
import ExtraParamsField from './ExtraParamsField';
import { mergeExtras } from './params';

type Props = ComponentProps<typeof ExtraParamsField>;

/** ExtraParamsField is controlled; this holds the text so typing is live. */
function Controlled({ values, onChange, ...rest }: Props) {
  const [v, setV] = useState<Record<string, string>>(values);
  return (
    <ExtraParamsField {...rest} values={v}
      onChange={(key, raw) => { setV(prev => ({ ...prev, [key]: raw })); onChange(key, raw); }} />
  );
}

// `extra_allowlist` exactly as models/fal.yaml declares it.
const RODIN = { extra_allowlist: ['quality', 'tier', 'use_hyper', 'TAPose', 'condition_mode', 'bbox_condition', 'addons'] } as unknown as ModelSchema;
const TRELLIS = { extra_allowlist: ['mesh_simplify', 'texture_size', 'ss_guidance_strength', 'ss_sampling_steps', 'slat_guidance_strength', 'slat_sampling_steps'] } as unknown as ModelSchema;

function selection(schemas: Record<string, ModelSchema>, values: Record<string, string> = {}, docsUrl?: string): Props {
  return { extras: mergeExtras(schemas), values, totalSelected: Object.keys(schemas).length, onChange: fn(), docsUrl };
}

/**
 * Provider passthrough: vendor fields a model allows in `extra` that no
 * canonical param maps to. Collapsed until a value is set.
 */
const meta = {
  title: 'Playground/ExtraParamsField',
  component: ExtraParamsField,
  render: args => <Controlled {...args} />,
  decorators: [Story => <div style={{ maxWidth: 380 }}><Story /></div>],
  args: selection({ 'fal/hyper3d-rodin': RODIN }, {}, 'https://fal.ai/models/fal-ai/hyper3d/rodin/api'),
} satisfies Meta<typeof ExtraParamsField>;
export default meta;
type Story = StoryObj<typeof meta>;

/** Nothing set yet: collapsed, with a count of the keys on offer. */
export const Collapsed: Story = {};

/** Values set: open, each noting how it will be sent (JSON vs text). */
export const WithValues: Story = {
  args: selection({ 'fal/hyper3d-rodin': RODIN }, { quality: 'high', use_hyper: 'true', bbox_condition: '[1, 1, 1]' },
    'https://fal.ai/models/fal-ai/hyper3d/rodin/api'),
};

/** Two models compared: each key notes which one it reaches. */
export const MixedSelection: Story = {
  args: selection({ 'fal/hyper3d-rodin': RODIN, 'fal/trellis': TRELLIS }, { texture_size: '1024' }),
};

// Typed keys (`extra_params`) in the shape GET /v1/models/{id} serves them for
// fal's FLUX.1 [pro] rows: `safety_tolerance` is the STRING "1".."6".
const FLUX_PRO = {
  extra_allowlist: ['raw', 'safety_tolerance', 'image_prompt_strength', 'enhance_prompt'],
  extra_params: {
    raw: { kind: 'bool', default: null },
    safety_tolerance: { kind: 'string', enum_values: ['1', '2', '3', '4', '5', '6'], default: '2' },
    image_prompt_strength: { kind: 'float', min: 0, max: 1, default: 0.1 },
    enhance_prompt: { kind: 'bool', default: null },
  },
} as unknown as ModelSchema;

/** Typed keys: each gets a control of its vendor type and goes as that type. */
export const TypedFields: Story = {
  args: selection({ 'fal/flux-pro': FLUX_PRO }, { safety_tolerance: '2', image_prompt_strength: '0.25' }),
};
