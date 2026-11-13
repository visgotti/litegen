// The ways a viewer can be stopped that the browser, not the file, decides:
// a WebGL context refused or taken back, a loader's code that never arrives,
// a download that never answers, and textures that are not there. The
// converter tool's stories project checks each by its tags
// (apps/3d-converter/e2e/stories.spec.ts; see RemoteMeshViewer.stories.tsx).
import type { ComponentType } from 'react';
import type { ViewerFile } from '../types';
import MeshViewer from './MeshViewer';
import RemoteMeshViewer from './RemoteMeshViewer';

const FIXTURES = '/viewer-fixtures';

export default {
  title: 'Shared/ViewerFailures',
  component: RemoteMeshViewer,
  args: { testId: 'story-viewer' },
  decorators: [(Story: ComponentType) => <div style={{ maxWidth: 640 }}><Story /></div>],
};

/** The browser answers the WebGL 2 probe but refuses the viewer's own
 *  context, as Chrome does once it blocks WebGL after GPU resets: three.js's
 *  renderer throws. The viewer must fall back, not take the page down. */
export const ContextRefused = {
  tags: ['viewer-error', 'reason-webgl'],
  args: { url: `${FIXTURES}/cube.glb`, format: 'glb' },
  loaders: [
    async () => {
      const original = HTMLCanvasElement.prototype.getContext;
      let given = 0;
      HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, ...rest: unknown[]) {
        if (type === 'webgl2' && given++ > 0) return null;
        return (original as (this: HTMLCanvasElement, type: string, ...rest: unknown[]) => RenderingContext | null).call(this, type, ...rest);
      } as typeof original;
      return {};
    },
  ],
};

/** The browser takes the first viewer context back while its model is still
 *  loading (a page holding more contexts than it keeps, a GPU reset). The
 *  viewer must say so and offer Retry, not report a blank canvas as drawn. */
export const ContextLostWhileLoading = {
  tags: ['viewer-error', 'reason-context'],
  args: { url: `${FIXTURES}/cube.glb`, format: 'glb' },
  loaders: [
    async () => {
      let done = false;
      new MutationObserver(() => {
        const canvas = document.querySelector<HTMLCanvasElement>('canvas.mv-canvas');
        if (done || !canvas) return;
        done = true;
        canvas.getContext('webgl2')?.getExtension('WEBGL_lose_context')?.loseContext();
      }).observe(document.body, { childList: true, subtree: true });
      return {};
    },
  ],
};

/** The GLB loader's own code never arrives (its chunk is blocked by the test,
 *  as a stale chunk is after a deploy). That is not a broken file: the
 *  viewer says the viewer could not load, and offers the download. */
export const LoaderUnavailable = {
  tags: ['viewer-error', 'reason-import', 'block-gltf-loader'],
  args: { url: `${FIXTURES}/cube.glb`, format: 'glb' },
};

/** The server never answers: the bar sweeps, since nothing says how much has
 *  arrived, rather than sitting at a made-up 0%. */
export const Pending = {
  tags: ['viewer-pending'],
  args: { url: `${FIXTURES}/cube.glb`, format: 'glb', fetch: () => new Promise<Response>(() => {}) },
};

async function filesOf(paths: Array<[string, ViewerFile['role']]>): Promise<ViewerFile[]> {
  return Promise.all(paths.map(async ([path, role]) => ({
    name: path.slice(path.lastIndexOf('/') + 1),
    data: new Uint8Array(await (await fetch(path)).arrayBuffer()),
    role,
  })));
}

/** An OBJ with its material library but none of the images the library
 *  names: drawn untextured as soon as those loads fail, not after waiting
 *  out a timeout for images that will never come. */
export const ObjMissingTextures = {
  tags: ['viewer', 'viewer-fast'],
  loaders: [async () => ({ files: await filesOf([[`${FIXTURES}/cube.obj`, 'model'], [`${FIXTURES}/cube.mtl`, 'material_library']]) })],
  render: (_: unknown, { loaded }: { loaded: { files: ViewerFile[] } }) => <MeshViewer files={loaded.files} format="obj" testId="story-viewer" />,
};
