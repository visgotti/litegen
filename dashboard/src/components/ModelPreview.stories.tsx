import type { Meta, StoryObj } from '@storybook/react-vite';
import ModelPreview from './ModelPreview';
import {
  CUBE_MESH, FULL_ASSETS, ICOSPHERE_MESH, MISSING_MESH, PREVIEW, TEXTURE, fixtureUrl,
} from '../story-fixtures';

/**
 * The full 3D inspector — Generations expanded row, Playground single mode,
 * and the Logs trace panel's Visual tab. Every state is reachable from props
 * alone (no network beyond the fixture files), which is what makes it storyable.
 */
const meta = {
  title: 'Observability/ModelPreview',
  component: ModelPreview,
  args: { testId: 'story-preview' },
  decorators: [Story => <div style={{ maxWidth: 820 }}><Story /></div>],
} satisfies Meta<typeof ModelPreview>;
export default meta;
type Story = StoryObj<typeof meta>;

/** What a textured provider returns: mesh, base-colour texture and a 2D
 *  preview — viewer, stats, thumbnails and the complete asset list. */
export const FullAssetSet: Story = {
  args: { assets: FULL_ASSETS },
};

/** Just a mesh, with no size or polycount reported: the stats fall back to
 *  dashes and there is no thumbnail strip. */
export const MeshOnly: Story = {
  args: { assets: [{ kind: 'mesh', url: CUBE_MESH.url, format: 'glb' }] },
};

/** A completed result with no mesh asset: a provider contract violation the
 *  inspector must name, while still listing what it did receive. */
export const NoMeshContractViolation: Story = {
  args: { assets: [TEXTURE, PREVIEW] },
};

/** A 5,120-triangle icosphere, so the polycount stat is non-trivial. */
export const HighPolycount: Story = {
  args: { assets: [ICOSPHERE_MESH, PREVIEW] },
};

/** The light background, selected through the toolbar the way a user would. */
export const LightBackground: Story = {
  args: { assets: FULL_ASSETS },
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(await canvas.findByTestId('story-preview-bg-light'));
  },
};

/** The transparency checkerboard background, selected through the toolbar. */
export const CheckerBackground: Story = {
  args: { assets: [ICOSPHERE_MESH, PREVIEW] },
  play: async ({ canvas, userEvent }) => {
    await userEvent.click(await canvas.findByTestId('story-preview-bg-checker'));
  },
};

/** The mesh URL 404s: the viewer falls back (download + Retry), viewer-only
 *  controls disable, and copy-URL and the asset links stay usable. */
export const MeshLoadFailure: Story = {
  args: { assets: [MISSING_MESH, PREVIEW] },
};

/** An OBJ mesh: known up front not to render in `<model-viewer>`, so the
 *  inspector reports it as unsupported and keeps the stats and downloads. */
export const UnsupportedFormat: Story = {
  args: {
    assets: [{ kind: 'mesh', url: fixtureUrl('does-not-exist/model.obj'), format: 'obj', size_bytes: 48213, polycount: 1204 }],
  },
};

/** A failure the caller already knows about replaces the viewer. */
export const CallerReportedError: Story = {
  args: { assets: [], error: 'Provider rejected the prompt: content policy.' },
};
