import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { formats, type Format, type StoredFrame } from '@litegen/3d-converter';
import { MeshViewer, type ViewerFile } from '@litegen/3d-viewer';
import { fetchFiles, text } from '../story-files';

/** The frame each format stores, from the converter package itself. */
const frameOf = (format: Format): StoredFrame => formats().find((f) => f.format === format)!.storedFrame;

/**
 * Every format through three.js's own loader for it. Tagged `viewer`, each
 * story must load and draw its model (e2e/stories.spec.ts).
 */
const meta = {
  title: 'Viewer/MeshViewer',
  component: MeshViewer,
  args: { files: [], format: 'glb', storedFrame: 'y_up', testId: 'story-viewer', onLoad: fn(), onError: fn() },
  decorators: [(Story) => <div style={{ maxWidth: 640 }}><Story /></div>],
  render: (args, { loaded }) => {
    const { files, format } = loaded as { files: ViewerFile[]; format: Format };
    return <MeshViewer {...args} files={files} format={format} storedFrame={frameOf(format)} />;
  },
} satisfies Meta<typeof MeshViewer>;
export default meta;
type Story = StoryObj<typeof meta>;

const from = (format: Format, paths: string[], main?: string): Story['loaders'] => [async () => ({ files: await fetchFiles(paths, main), format })];

/** A textured cube (the dashboard's Storybook mesh), as glTF Binary. */
export const Glb: Story = { tags: ['viewer'], loaders: from('glb', ['/dashboard-fixtures/cube.glb']) };

/** A higher-poly metallic mesh. */
export const GlbIcosphere: Story = { tags: ['viewer'], loaders: from('glb', ['/dashboard-fixtures/icosphere.glb']) };

/** Binary FBX 7500, written by assimp. */
export const Fbx: Story = { tags: ['viewer'], loaders: from('fbx', ['/testdata/fbx/textured_assimp_7500.fbx']) };

/** ASCII FBX 7.5, written by assimp. */
export const FbxAscii: Story = { tags: ['viewer'], loaders: from('fbx', ['/testdata/fbx/cube_assimp_ascii.fbx']) };

/** A skinned COLLADA file written by Apple's SceneKit. */
export const Dae: Story = { tags: ['viewer'], loaders: from('dae', ['/testdata/dae/scenekit/skinned_scenekit.dae']) };

/** A textured 3MF written by assimp: stored +Z up, turned upright. */
export const ThreeMf: Story = { tags: ['viewer'], loaders: from('3mf', ['/testdata/3mf/assimp_textured.3mf']) };

/** A textured USDZ written by Apple's USD tools. */
export const Usdz: Story = { tags: ['viewer'], loaders: from('usdz', ['/testdata/usd/textured_apple.usdz']) };

/** A binary USD crate (read as usda), written by Pixar's usdcat. */
export const UsdCrate: Story = { tags: ['viewer'], loaders: from('usda', ['/testdata/usd/cube.usdc']) };

/** A big-endian binary PLY. */
export const Ply: Story = { tags: ['viewer'], loaders: from('ply', ['/testdata/ply/cube_be_struct.ply']) };

/** glTF as three files: the model, its .bin and its texture (the
 *  textured-gltf sample). */
export const Gltf: Story = {
  tags: ['viewer'],
  loaders: from('gltf', ['/samples/textured-gltf/model.gltf', '/samples/textured-gltf/model.bin', '/samples/textured-gltf/quadrants.png'], 'model.gltf'),
};

/** The OBJ package the converter writes: the model, its .mtl and its texture
 *  (the textured-obj sample). */
export const Obj: Story = {
  tags: ['viewer'],
  loaders: from('obj', ['/samples/textured-obj/model.obj', '/samples/textured-obj/model.mtl', '/samples/textured-obj/textures/texture_0.png'], 'model.obj'),
};

/** A small OBJ, with no material library: the loader's default material. */
export const ObjPlain: Story = {
  tags: ['viewer'],
  loaders: [async () => ({
    format: 'obj',
    files: [text('pyramid.obj', 'v 0 0 0\nv 1 0 0\nv 1 0 1\nv 0 0 1\nv 0.5 1 0.5\nf 1 2 3\nf 1 3 4\nf 1 2 5\nf 2 3 5\nf 3 4 5\nf 4 1 5\n')],
  })],
};

/** An ASCII STL, which carries no units, no up axis and no material. */
export const Stl: Story = {
  tags: ['viewer'],
  loaders: [async () => ({
    format: 'stl',
    files: [text('wedge.stl', 'solid wedge\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nfacet normal 0 -1 0\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 0 1\nendloop\nendfacet\nendsolid wedge\n')],
  })],
};

/** While three.js parses a model: a 500,000-triangle OBJ grid shows the
 *  progress bar for a moment, then the model. */
export const Loading: Story = {
  tags: ['viewer'],
  loaders: [async () => {
    const n = 500;
    const lines: string[] = [];
    for (let y = 0; y <= n; y++) for (let x = 0; x <= n; x++) lines.push(`v ${x / n} ${Math.sin(x / 20) * Math.cos(y / 20) * 0.05} ${y / n}`);
    for (let y = 0; y < n; y++) {
      for (let x = 0; x < n; x++) {
        const a = y * (n + 1) + x + 1;
        lines.push(`f ${a} ${a + n + 1} ${a + 1}`, `f ${a + 1} ${a + n + 1} ${a + n + 2}`);
      }
    }
    return { format: 'obj', files: [text('grid.obj', `${lines.join('\n')}\n`)] };
  }],
};

/** A browser without WebGL 2: the fallback and its message. The probe runs
 *  once per page, so open this story on its own (as the smoke test does). */
export const NoWebGL: Story = {
  tags: ['viewer-error'],
  decorators: [
    (Story) => {
      const getContext = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, type: string, ...rest: unknown[]) {
        return type === 'webgl2' ? null : (getContext as (...a: unknown[]) => unknown).call(this, type, ...rest);
      } as typeof getContext;
      return <Story />;
    },
  ],
  loaders: from('glb', ['/dashboard-fixtures/cube.glb']),
};

/** A file that is not what its format says: the fallback, with a download. */
export const Corrupt: Story = { tags: ['viewer-error'], loaders: from('glb', ['/dashboard-fixtures/corrupt.glb']) };

/** The light background, for contrast. */
export const LightBackground: Story = { tags: ['viewer'], args: { background: 'light' }, loaders: from('glb', ['/dashboard-fixtures/cube.glb']) };

/** The checkerboard background, still. */
export const CheckerStill: Story = { tags: ['viewer'], args: { background: 'checker', autoRotate: false }, loaders: from('glb', ['/dashboard-fixtures/icosphere.glb']) };
