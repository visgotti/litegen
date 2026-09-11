import type { Meta, StoryObj } from '@storybook/react-vite';
import { MediaPreview } from './Generations';
import { CUBE_MESH, FULL_ASSETS, IMAGE_URL, TEXTURE, demoClip, generation } from '../story-fixtures';

/**
 * The Generations page's per-row media switch: `thumb` is the 48px cell in the
 * table row, full mode is the expanded detail row. Image, video and 3D (with
 * and without a preview render) in each.
 */
const meta = {
  title: 'Observability/Generations MediaPreview',
  component: MediaPreview,
  decorators: [Story => <div style={{ maxWidth: 820 }}><Story /></div>],
} satisfies Meta<typeof MediaPreview>;
export default meta;
type Story = StoryObj<typeof meta>;

const image = generation({
  id: 'gen_story_image_0001', model: 'mock/image-gen', media_type: 'image', result_url: IMAGE_URL, metadata: null,
});
const video = (url: string) => generation({
  id: 'gen_story_video_0001', model: 'mock/video-gen', media_type: 'video', result_url: url, metadata: null,
});
const withPreview = generation();
/** Mesh and texture, no `preview` asset: the thumbnail has no image to show. */
const withoutPreview = generation({ id: 'gen_story_3d_0002', metadata: { assets: [CUBE_MESH, TEXTURE] } });

const clipLoader = async () => ({ clip: await demoClip() });
const noClip = <p style={{ color: '#8b949e' }}>This browser cannot record the demo clip (no MediaRecorder).</p>;

export const ThumbImage: Story = { args: { g: image, thumb: true } };

/** Videos never get a thumbnail frame — a glyph stands in. */
export const ThumbVideo: Story = { args: { g: video(IMAGE_URL), thumb: true } };

/** The provider's preview render is the 3D thumbnail. */
export const Thumb3DWithPreview: Story = { args: { g: withPreview, thumb: true } };

/** No preview render: a glyph, not a GL canvas per table row. */
export const Thumb3DWithoutPreview: Story = { args: { g: withoutPreview, thumb: true } };

/** A row still in flight has no media yet. */
export const ThumbInFlight: Story = {
  args: { g: generation({ status: 'processing', progress: 66, result_url: null, metadata: null }), thumb: true },
};

export const FullImage: Story = { args: { g: image } };

/** A real WebM clip, recorded in the browser by a loader. */
export const FullVideo: Story = {
  args: { g: video(IMAGE_URL) },
  loaders: [clipLoader],
  render: (args, { loaded }) => (loaded.clip ? <MediaPreview {...args} g={video(loaded.clip as string)} /> : noClip),
};

/** The expanded row mounts the full inspector over the row's assets. */
export const Full3DWithPreview: Story = { args: { g: withPreview } };

/** Without a preview there is no poster or preview thumbnail; the texture
 *  still gets its thumbnail and the viewer renders the mesh. */
export const Full3DWithoutPreview: Story = { args: { g: withoutPreview } };

/** Assets metadata written before `assets[]` existed: the mesh is recovered
 *  from `result_url` alone. */
export const Full3DLegacyResultUrlOnly: Story = {
  args: { g: generation({ id: 'gen_story_3d_legacy', metadata: null, result_url: FULL_ASSETS[0].url }) },
};
