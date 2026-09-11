import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import ResultGrid from './ResultGrid';
import type { ResultTileState } from './types';
import { CUBE_MESH, FULL_ASSETS, IMAGE_URL } from '../story-fixtures';

const threeD: ResultTileState = {
  key: 'mock/mesh-3d#0',
  modelId: 'mock/mesh-3d',
  index: 0,
  status: 'done',
  mediaType: 'model3d',
  request: { model: 'mock/mesh-3d', prompt: 'a low-poly fox' },
};

/** The Compare-mode result grid, which picks `ResultTile3D` or `ResultTile`
 *  per tile by media type. */
const meta = {
  title: 'Playground/ResultGrid',
  component: ResultGrid,
  args: { tiles: [], onRerun: fn() },
  decorators: [Story => <div style={{ maxWidth: 1100 }}><Story /></div>],
} satisfies Meta<typeof ResultGrid>;
export default meta;
type Story = StoryObj<typeof meta>;

/** Mid-run: a finished 3D tile, one still polling, one failed, beside an
 *  image model's tile. */
export const MixedCompareRun: Story = {
  args: {
    tiles: [
      { ...threeD, latencyMs: 4200, costUsd: 0.04, assets: FULL_ASSETS, url: CUBE_MESH.url },
      { ...threeD, key: 'mock/all-params-3d#0', modelId: 'mock/all-params-3d', status: 'polling', progress: 66 },
      { ...threeD, key: 'mock/fail-3d#0', modelId: 'mock/fail-3d', status: 'error', error: 'mock 3d generation failed' },
      {
        ...threeD, key: 'mock/image-gen#0', modelId: 'mock/image-gen', mediaType: 'image',
        url: IMAGE_URL, latencyMs: 900, costUsd: 0.01, request: { model: 'mock/image-gen', prompt: 'a low-poly fox' },
      },
    ],
  },
};

export const Empty: Story = {};
