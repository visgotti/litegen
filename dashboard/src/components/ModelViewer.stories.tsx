import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import ModelViewer from './ModelViewer';
import {
  CORRUPT_MESH, CUBE_MESH, ICOSPHERE_MESH, MISSING_MESH, PREVIEW, STALLED_MESH_URL, fixtureUrl, installStalledMeshRoute,
} from '../story-fixtures';

/**
 * The compact `<model-viewer>` wrapper used by Playground grid tiles and the
 * trace panel, and inside the `ModelPreview` inspector.
 *
 * Not storied: the **bundle-import failure** (`data-reason="import"`). The
 * component imports `@google/model-viewer` with a bare dynamic `import()` and
 * offers no seam to make that fail, and adding a production hook purely for a
 * story is not worth it. The state is exercised instead by the real-browser
 * pass, which blocks the model-viewer chunk request (see the Task 20 report).
 * Likewise the missing-WebGL-2 state (`data-reason="webgl"`), which is probed
 * once per page from the real browser.
 */
const meta = {
  title: 'Observability/ModelViewer',
  component: ModelViewer,
  args: { testId: 'story-viewer', onLoad: fn(), onError: fn(), onRetry: fn() },
  decorators: [Story => <div style={{ maxWidth: 640 }}><Story /></div>],
} satisfies Meta<typeof ModelViewer>;
export default meta;
type Story = StoryObj<typeof meta>;

/** A textured cube, fully loaded and auto-rotating. */
export const Loaded: Story = {
  args: { src: CUBE_MESH.url, format: 'glb' },
};

/** A higher-poly mesh with a metallic material and the preview as poster. */
export const LoadedHighPoly: Story = {
  args: { src: ICOSPHERE_MESH.url, format: 'glb', poster: PREVIEW.url },
};

/**
 * Mid-download: the poster shows and the progress bar sits at the mesh's real
 * streamed fraction. The download is held open by a story-only `fetch`
 * interceptor, because in the live app this state lasts only as long as the
 * transfer does.
 */
export const Loading: Story = {
  args: { src: STALLED_MESH_URL, format: 'glb', poster: PREVIEW.url },
  loaders: [async () => { installStalledMeshRoute(); return {}; }],
};

/** The mesh URL 404s: the viewer degrades to a download link plus Retry. */
export const MeshNotFound: Story = {
  args: { src: MISSING_MESH.url, format: 'glb' },
};

/** A GLB whose JSON chunk is truncated: the file arrives but cannot parse. */
export const CorruptGlb: Story = {
  args: { src: CORRUPT_MESH.url, format: 'glb' },
};

/** `<model-viewer>` is glTF-only, so an OBJ is never fetched — straight to
 *  the download fallback (no Retry: a second attempt cannot help). */
export const NonGltfFormat: Story = {
  args: { src: fixtureUrl('does-not-exist/model.obj'), format: 'obj' },
};

/** The inspector's light background, applied to the compact viewer. */
export const LightBackground: Story = {
  args: { src: CUBE_MESH.url, format: 'glb', background: 'light' },
};
