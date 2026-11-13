import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import ModelViewer from './ModelViewer';
import {
  CORRUPT_MESH, CUBE_MESH, ICOSPHERE_MESH, MISSING_MESH, PREVIEW, STALLED_MESH_URL, fixtureUrl, installStalledMeshRoute, viewerFixtureUrl,
} from '../story-fixtures';

/**
 * The compact viewer used by the Playground's compare tiles: the shared
 * viewer (@litegen/3d-viewer), the same one the 3D converter tool shows,
 * downloading the mesh itself. Its own states are storied in the package
 * (Shared/RemoteMeshViewer); these are the dashboard's meshes through the
 * dashboard's adapter.
 *
 * Not storied: missing WebGL 2 (`data-reason="webgl"`), probed once per page
 * from the real browser.
 */
const meta = {
  title: 'Observability/ModelViewer',
  component: ModelViewer,
  args: { testId: 'story-viewer', onLoad: fn(), onError: fn() },
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

/** A GLB whose JSON chunk is truncated: the file arrives but cannot parse,
 *  so only the download link is offered. */
export const CorruptGlb: Story = {
  args: { src: CORRUPT_MESH.url, format: 'glb' },
};

/** An OBJ result, which `<model-viewer>` could not show: drawn by three.js's
 *  OBJ loader, as every format litegen delivers is. */
export const ObjMesh: Story = {
  args: { src: viewerFixtureUrl('cube.obj'), format: 'obj' },
};

/** A container the viewer has no loader for: never downloaded, straight to
 *  the download fallback (no Retry: a second attempt cannot help). */
export const UnknownFormat: Story = {
  args: { src: fixtureUrl('does-not-exist/model.xyz'), format: 'xyz' },
};

/** The inspector's light background, applied to the compact viewer. */
export const LightBackground: Story = {
  args: { src: CUBE_MESH.url, format: 'glb', background: 'light' },
};
