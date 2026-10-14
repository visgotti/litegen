import { useState, type ComponentProps } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import type { ModelSchema } from '@litegen/sdk';
import { fn } from 'storybook/test';
import RefImagesField from './RefImagesField';
import { maxTotalRefs, mergeRefRoles } from './params';
import type { RefEntry } from './types';
import { fixtureUrl } from '../story-fixtures';

type Props = ComponentProps<typeof RefImagesField>;

/** RefImagesField is controlled; this holds the list so adding/removing is live. */
function Controlled({ refs, onChange, ...rest }: Props) {
  const [v, setV] = useState<RefEntry[]>(refs);
  return <RefImagesField {...rest} refs={v} onChange={next => { setV(next); onChange(next); }} />;
}

// `ref_inputs` exactly as the catalog declares them (models/fal.yaml, mock.yaml).
const TRELLIS = {
  ref_inputs: { max_total: 1, default_role: 'init', roles: { init: { required: true, min_count: 1, max_count: 1 } } },
} as unknown as ModelSchema;
const RODIN = {
  ref_inputs: { max_total: 5, default_role: 'init', roles: { init: { required: false, min_count: 0, max_count: 5 } } },
} as unknown as ModelSchema;
const ALL_PARAMS = {
  ref_inputs: {
    max_total: 6,
    default_role: 'init',
    roles: Object.fromEntries(
      ['init', 'view-front', 'view-back', 'view-left', 'view-right', 'view-top']
        .map(r => [r, { required: false, min_count: 0, max_count: 1 }]),
    ),
  },
} as unknown as ModelSchema;

/** Story args for a selection of models, as the Playground merges them. */
function selection(schemas: Record<string, ModelSchema>, refs: RefEntry[] = []): Props {
  return {
    roles: mergeRefRoles(schemas),
    maxTotal: maxTotalRefs(schemas),
    totalSelected: Object.keys(schemas).length,
    refs,
    onChange: fn(),
  };
}

const urlRef = (role: string): RefEntry => ({ id: `${role}-url`, role, type: 'url', value: fixtureUrl('image.png') });

/**
 * Reference images for the selected models — one block per role they accept.
 * A URL is fetched by the server; an upload travels inline as base64. Each
 * model is sent only the roles it declares.
 */
const meta = {
  title: 'Playground/RefImagesField',
  component: RefImagesField,
  render: args => <Controlled {...args} />,
  decorators: [Story => <div style={{ maxWidth: 380 }}><Story /></div>],
  args: selection({ 'fal/trellis': TRELLIS }),
} satisfies Meta<typeof RefImagesField>;
export default meta;
type Story = StoryObj<typeof meta>;

/** An image-only model: the input image is required, and only one. */
export const ImageOnlyModel: Story = {};

/** The same, with the image attached — the add row is gone at the cap. */
export const ImageAttached: Story = { args: selection({ 'fal/trellis': TRELLIS }, [urlRef('init')]) };

/** Text-or-image (Rodin): optional, up to five. */
export const OptionalUpToFive: Story = {
  args: selection({ 'fal/hyper3d-rodin': RODIN }, [urlRef('init')]),
};

/** Multiview: one block per view, all optional. */
export const Multiview: Story = { args: selection({ 'mock/all-params-3d': ALL_PARAMS }) };

/** TRELLIS and Rodin compared: the strictest cap wins and "required by" names who needs it. */
export const MixedSelection: Story = {
  args: selection({ 'fal/trellis': TRELLIS, 'fal/hyper3d-rodin': RODIN }),
};
