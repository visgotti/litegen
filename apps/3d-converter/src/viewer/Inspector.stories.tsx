import type { Meta, StoryObj } from '@storybook/react-vite';
import { formats, readModel, type Format } from '@litegen/3d-converter';
import { ModelInspector } from '@litegen/3d-viewer';
import ModelFacts from '../components/ModelFacts';
import { fetchFiles } from '../story-files';

/** The shared inspector (@litegen/3d-viewer), as the tool fills it. */
const meta = {
  title: 'Viewer/Inspector',
  component: ModelInspector,
  decorators: [(Story) => <div style={{ maxWidth: 820 }}><Story /></div>],
  render: (_, { loaded }) => {
    const { files, format, info } = loaded as { files: Awaited<ReturnType<typeof fetchFiles>>; format: Format; info: ReturnType<typeof readModel>['info'] };
    const formatInfo = formats().find((f) => f.format === format)!;
    const size = files.reduce((n, f) => n + f.data.length, 0);
    const b = info.bounds;
    return (
      <ModelInspector
        title="Input"
        testId="inspector"
        source={{ files }}
        format={format}
        formatLabel={formatInfo.label}
        storedFrame={formatInfo.storedFrame}
        size={size}
        triangles={info.triangles}
        dimensions={b ? { x: b.max[0] - b.min[0], y: b.max[1] - b.min[1], z: b.max[2] - b.min[2] } : null}
      >
        <ModelFacts info={info} testId="inspector" />
      </ModelInspector>
    );
  },
} satisfies Meta<typeof ModelInspector>;
export default meta;
type Story = StoryObj<typeof meta>;

const inspect = (format: Format, path: string): Story['loaders'] => [async () => {
  const files = await fetchFiles([path]);
  const m = readModel(files[0]!.data, { from: format });
  const info = m.info;
  m.free();
  return { files, format, info };
}];

/** A skinned, animated glb: the animation toggle appears. */
export const AnimatedGlb: Story = { tags: ['viewer'], args: {} as never, loaders: inspect('glb', '/testdata/fbx/skinned_small_source.glb') };

/** Apple's textured USDZ. */
export const TexturedUsdz: Story = { tags: ['viewer'], args: {} as never, loaders: inspect('usdz', '/testdata/usd/textured_apple.usdz') };

/** assimp's textured binary FBX. */
export const TexturedFbx: Story = { tags: ['viewer'], args: {} as never, loaders: inspect('fbx', '/testdata/fbx/textured_assimp_7500.fbx') };
