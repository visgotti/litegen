import type { Meta, StoryObj } from '@storybook/react-vite';
import { convert, readModel } from '@litegen/3d-converter';
import ModelFacts from './ModelFacts';
import LossList from './LossList';
import FileList from './FileList';

/** The inspector's panels, over real results of the converter package. */
const meta = {
  title: 'Panels',
  decorators: [(Story) => <section className="mp" style={{ maxWidth: 760 }}><Story /></section>],
} satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

const bytes = async (path: string) => new Uint8Array(await (await fetch(path)).arrayBuffer());

/** A skinned, animated model's facts and clips. */
export const FactsSkinned: Story = {
  loaders: [async () => {
    const m = readModel(await bytes('/testdata/fbx/skinned_small_source.glb'));
    const info = m.info;
    m.free();
    return { info };
  }],
  render: (_, { loaded }) => <ModelFacts info={loaded.info} testId="facts" />,
};

/** A textured, static model's facts. */
export const FactsTextured: Story = {
  loaders: [async () => {
    const m = readModel(await bytes('/testdata/usd/textured_apple.usdz'));
    const info = m.info;
    m.free();
    return { info };
  }],
  render: (_, { loaded }) => <ModelFacts info={loaded.info} testId="facts" />,
};

/** A skinned, animated model converted to STL: what STL could not keep. */
export const LossesToStl: Story = {
  loaders: [async () => ({ conversion: convert(await bytes('/testdata/fbx/skinned_small_source.glb'), { to: 'stl' }) })],
  render: (_, { loaded }) => <LossList losses={loaded.conversion.losses} warnings={loaded.conversion.warnings} testId="losses" />,
};

/** The same model to glTF Binary: nothing lost. */
export const LossesNone: Story = {
  loaders: [async () => ({ conversion: convert(await bytes('/testdata/fbx/skinned_small_source.glb'), { to: 'glb' }) })],
  render: (_, { loaded }) => <LossList losses={loaded.conversion.losses} warnings={loaded.conversion.warnings} testId="losses" />,
};

/** An OBJ package's files and their roles. */
export const FilesObjPackage: Story = {
  loaders: [async () => ({ conversion: convert(await bytes('/testdata/fbx/textured_assimp_7500.fbx'), { to: 'obj', package: 'files' }) })],
  render: (_, { loaded }) => (
    <FileList title="Files" testId="files" files={loaded.conversion.files.map((f: { name: string; role: 'model'; data: Uint8Array }) => ({ name: f.name, role: f.role, size: f.data.length }))} />
  ),
};
