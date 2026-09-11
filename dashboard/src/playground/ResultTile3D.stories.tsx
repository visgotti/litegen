import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import ResultTile3D from './ResultTile3D';
import type { ResultTileState } from './types';
import { CUBE_MESH, FULL_ASSETS } from '../story-fixtures';

/**
 * A Compare-mode grid tile for a 3D model. Every `TileStatus` is here, because
 * the polling states are transient in the live Playground.
 */
const base: ResultTileState = {
  key: 'mock/mesh-3d#0',
  modelId: 'mock/mesh-3d',
  index: 0,
  status: 'queued',
  mediaType: 'model3d',
  request: { model: 'mock/mesh-3d', prompt: 'a low-poly fox' },
};

const meta = {
  title: 'Playground/ResultTile3D',
  component: ResultTile3D,
  args: { tile: base, onRerun: fn() },
  // The width of one cell in the Compare grid (`.pg-grid`, minmax 200px).
  decorators: [Story => <div className="pg-grid" style={{ maxWidth: 300 }}><Story /></div>],
} satisfies Meta<typeof ResultTile3D>;
export default meta;
type Story = StoryObj<typeof meta>;

export const Queued: Story = {};

/** Submitting the job (the request is in flight, no job id yet). */
export const Running: Story = { args: { tile: { ...base, status: 'running' } } };

/** Accepted and polling the server-side job — the state 3D introduced to a
 *  Playground that had no async path. */
export const Polling33: Story = { args: { tile: { ...base, status: 'polling', progress: 33 } } };
export const Polling66: Story = { args: { tile: { ...base, status: 'polling', progress: 66 } } };

/** Completed: the compact viewer with the preview render as its poster. */
export const Done: Story = {
  args: {
    tile: { ...base, status: 'done', progress: 100, latencyMs: 4200, costUsd: 0.04, assets: FULL_ASSETS, url: CUBE_MESH.url },
  },
};

export const Failed: Story = {
  args: { tile: { ...base, status: 'error', error: 'mock 3d generation failed', latencyMs: 2100 } },
};

/**
 * `done` with no mesh asset. The live fan-out converts this to an error tile
 * ("completed without a mesh asset"); this is the tile's own fallback if a
 * mesh-less `done` ever reaches it.
 */
export const DoneWithoutMesh: Story = {
  args: { tile: { ...base, status: 'done', assets: [], latencyMs: 4200, costUsd: 0 } },
};

/** What the live fan-out produces for the same contract violation. */
export const CompletedWithoutMeshAsError: Story = {
  args: { tile: { ...base, status: 'error', error: 'completed without a mesh asset', latencyMs: 4200, costUsd: 0 } },
};

/** The user hit Cancel mid-poll: the fan-out sweeps the tile to this error. */
export const Cancelled: Story = {
  args: { tile: { ...base, status: 'error', error: 'cancelled' } },
};
