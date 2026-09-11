import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import ResultTile from './ResultTile';
import type { ResultTileState } from './types';
import { IMAGE_URL } from '../story-fixtures';

/** The image-model Compare tile, for contrast with `ResultTile3D`: it has no
 *  polling state, because image generation answers synchronously. */
const base: ResultTileState = {
  key: 'mock/image-gen#0',
  modelId: 'mock/image-gen',
  index: 0,
  status: 'done',
  mediaType: 'image',
  request: { model: 'mock/image-gen', prompt: 'a low-poly fox' },
};

const meta = {
  title: 'Playground/ResultTile (image)',
  component: ResultTile,
  args: { tile: base, onRerun: fn() },
  decorators: [Story => <div className="pg-grid" style={{ maxWidth: 300 }}><Story /></div>],
} satisfies Meta<typeof ResultTile>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Image: Story = {
  args: { tile: { ...base, url: IMAGE_URL, latencyMs: 900, costUsd: 0.01 } },
};

export const Generating: Story = { args: { tile: { ...base, status: 'running' } } };

export const Failed: Story = {
  args: { tile: { ...base, status: 'error', error: 'content policy violation' } },
};
