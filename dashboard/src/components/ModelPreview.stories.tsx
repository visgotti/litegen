import type { Meta, StoryObj } from '@storybook/react-vite';
import ModelPreview from './ModelPreview';
import {
  CORRUPT_MESH, CUBE_MESH, FULL_ASSETS, ICOSPHERE_MESH, MISSING_MESH, PREVIEW, TEXTURE, fixtureUrl, viewerFixtureUrl,
} from '../story-fixtures';

/**
 * The full 3D inspector — Generations expanded row, Playground single mode,
 * and the Logs trace panel's Visual tab: the shared inspector
 * (@litegen/3d-viewer), the same one the 3D converter tool shows, filled from
 * the API's assets. Every state is reachable from props alone (no network
 * beyond the fixture files), which is what makes it storyable.
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

/** A 5,120-triangle icosphere, so the triangles stat is non-trivial. */
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

/** A GLB that downloads fine but fails to parse (corrupt JSON chunk): the
 *  inspector shows the `parse` fallback (download only, no Retry — refetching
 *  the same bytes would fail the same way), while treating the error like any
 *  other: viewer-only controls disable, copy-URL and the asset list stay
 *  usable. */
export const MeshParseFailure: Story = {
  args: { assets: [CORRUPT_MESH, PREVIEW] },
};

/** An OBJ result, which `<model-viewer>` could not show: drawn by three.js's
 *  OBJ loader, with its stats and downloads. */
export const ObjMesh: Story = {
  args: {
    assets: [{ kind: 'mesh', url: viewerFixtureUrl('cube.obj'), format: 'obj', size_bytes: 11092 }],
  },
};

/** A 3MF result, stored +Z up. The API does not yet say which formats are
 *  stored so, and the dashboard knows no format's axis: it draws lying on
 *  its back until the formats endpoint gains `details` (a follow-up). */
export const ThreeMfMesh: Story = {
  args: {
    assets: [{ kind: 'mesh', url: viewerFixtureUrl('textured.3mf'), format: '3mf', size_bytes: 1786 }],
  },
};

/** A container the viewer has no loader for: the fallback, with the stats
 *  and downloads still there. */
export const UnknownFormat: Story = {
  args: {
    assets: [{ kind: 'mesh', url: fixtureUrl('does-not-exist/model.xyz'), format: 'xyz', size_bytes: 48213, polycount: 1204 }],
  },
};

/** A failure the caller already knows about replaces the viewer. */
export const CallerReportedError: Story = {
  args: { assets: [], error: 'Provider rejected the prompt: content policy.' },
};
