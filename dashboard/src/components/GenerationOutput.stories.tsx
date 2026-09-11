import type { Meta, StoryObj } from '@storybook/react-vite';
import GenerationOutput from './GenerationOutput';
import { IMAGE_URL, MISSING_VIDEO_URL, demoClip, generation } from '../story-fixtures';

/**
 * The Logs → trace panel's rendering of an async (video / 3D) request, driven
 * by the generation row the panel polls. Presentational: every state here is
 * a prop combination; `TracePanel` owns the fetching.
 */
const meta = {
  title: 'Observability/GenerationOutput',
  component: GenerationOutput,
  args: { loading: false },
  decorators: [Story => <div style={{ maxWidth: 820 }}><Story /></div>],
} satisfies Meta<typeof GenerationOutput>;
export default meta;
type Story = StoryObj<typeof meta>;

const video = (overrides: Parameters<typeof generation>[0] = {}) => generation({
  id: 'gen_story_video_0001',
  model: 'mock/video-gen',
  media_type: 'video',
  metadata: null,
  ...overrides,
});

/** Accepted, not yet started. */
export const Pending: Story = {
  args: { generation: generation({ status: 'pending', progress: 0, result_url: null, metadata: null, completed_at: null }) },
};

/** Two-thirds of the way through a provider job. */
export const Processing66: Story = {
  args: { generation: generation({ status: 'processing', progress: 66, result_url: null, metadata: null, completed_at: null }) },
};

/** One poll failed transiently: the last known state stays, with a note. */
export const Reconnecting: Story = {
  args: {
    generation: generation({ status: 'processing', progress: 66, result_url: null, metadata: null, completed_at: null }),
    reconnecting: true,
  },
};

/** A completed 3D generation: the full inspector over the row's assets. */
export const Completed3D: Story = {
  args: { generation: generation() },
};

/** A completed video generation, playing a real WebM clip (recorded in the
 *  browser by a loader, since the fixture script cannot encode video). */
export const CompletedVideo: Story = {
  args: { generation: null },
  loaders: [async () => ({ clip: await demoClip() })],
  render: (args, { loaded }) => (loaded.clip
    ? <GenerationOutput {...args} generation={video({ result_url: loaded.clip as string })} />
    : <p style={{ color: '#8b949e' }}>This browser cannot record the demo clip (no MediaRecorder).</p>),
};

/** A video generation whose output is an image (an animated GIF from a mock
 *  provider): rendered as `<img>`, not a broken `<video>`. */
export const CompletedVideoAsImage: Story = {
  args: { generation: video({ result_url: IMAGE_URL }) },
};

/** The video URL 404s (an expired signed URL, say): an explicit message and
 *  a direct link, never a silently broken player. */
export const VideoLoadFailure: Story = {
  args: { generation: video({ result_url: MISSING_VIDEO_URL }) },
};

/** The provider reported a failure. */
export const Failed: Story = {
  args: {
    generation: generation({
      status: 'failed', progress: 40, result_url: null, metadata: null,
      error_message: 'Provider job failed: mesh reconstruction did not converge.',
    }),
  },
};

/** Cancelled by the user, with the recorded reason. */
export const Cancelled: Story = {
  args: {
    generation: generation({
      status: 'cancelled', progress: 20, result_url: null, metadata: null, error_message: 'Cancelled via the dashboard.',
    }),
  },
};

/** The first fetch of the generation row is in flight. */
export const Resolving: Story = {
  args: { generation: null, loading: true },
};

/** The row does not exist yet and the caller is still retrying. */
export const WaitingForRecord: Story = {
  args: { generation: null, loading: false },
};

/** The caller gave up (a non-transient error, or its retry budget ran out). */
export const LoadError: Story = {
  args: { generation: null, error: 'HTTP 403: this generation belongs to another organization.' },
};

/** A status added server-side after this build must say so, not render nothing. */
export const UnknownStatus: Story = {
  args: { generation: generation({ status: 'archived' as never }) },
};
