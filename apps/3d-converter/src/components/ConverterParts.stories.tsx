import type { Meta, StoryObj } from '@storybook/react-vite';
import { useState } from 'react';
import { fn } from 'storybook/test';
import { convert, formats, type Format } from '@litegen/3d-converter';
import DropZone from './DropZone';
import SampleGallery from './SampleGallery';
import FormatPicker from './FormatPicker';
import OptionsPanel, { type Options } from './OptionsPanel';
import Downloads from './Downloads';
import '../App.css';

/** The converter page's parts, each on its own. */
const meta = { title: 'Converter/Parts', decorators: [(Story) => <div style={{ maxWidth: 720 }}><Story /></div>] } satisfies Meta;
export default meta;
type Story = StoryObj<typeof meta>;

/** Drop files or a folder, or pick them. */
export const Drop: Story = { render: () => <DropZone busy={false} onFiles={fn()} /> };

/** One sample per source format. */
export const Samples: Story = { render: () => <SampleGallery busy={false} current="textured-obj" onPick={fn()} /> };

function Picker() {
  const [value, setValue] = useState<Format | null>('usdz');
  return <FormatPicker formats={formats()} value={value} onChange={setValue} packaged={false} busy={false} />;
}
/** The ten targets, each saying what it keeps (from the package). */
export const Targets: Story = { render: () => <Picker /> };

function Panel({ from, to, start }: { from: Format; to: Format; start: Options }) {
  const all = formats();
  const [value, setValue] = useState<Options>(start);
  return <OptionsPanel source={all.find((f) => f.format === from)!} target={all.find((f) => f.format === to)!} value={value} onChange={setValue} busy={false} />;
}
/** An STL in, an OBJ out: units and up axis at both ends, and packaging. */
export const OptionsStlToObj: Story = { render: () => <Panel from="stl" to="obj" start={{ package: 'zip', fromUnits: 'mm', up: 'z' }} /> };

/** An FBX in, an FBX out: the colour space and the frame rate. */
export const OptionsFbxToFbx: Story = { render: () => <Panel from="fbx" to="fbx" start={{ package: 'file', fbxColorSpace: 'linear', fps: 24 }} /> };

/** A package's files as downloads, named as the model references them. */
export const DownloadsPackage: Story = {
  loaders: [async () => ({ conversion: convert(new Uint8Array(await (await fetch('/testdata/fbx/textured_assimp_7500.fbx')).arrayBuffer()), { to: 'obj', package: 'files' }) })],
  render: (_, { loaded }) => <Downloads conversion={loaded.conversion} stem="textured" />,
};
