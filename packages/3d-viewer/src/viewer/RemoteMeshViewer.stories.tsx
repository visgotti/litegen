// Stories for the viewer as the dashboard uses it: a model at a URL. Both
// Storybooks (the dashboard's and apps/3d-converter's) show them, and the
// converter tool's stories project checks each by its tags
// (apps/3d-converter/e2e/stories.spec.ts):
//   - `viewer`: draws its model;
//   - `viewer-downloading`: holds at 40% downloaded, poster showing;
//   - `viewer-error` with `reason-<why>`: ends in that failure, with Retry
//     only for `load`;
//   - `viewer-many`: never holds more WebGL contexts than a browser keeps.
// Plain CSF objects: no Storybook import, so the package needs no Storybook.
import type { ComponentType } from 'react';
import RemoteMeshViewer from './RemoteMeshViewer';

const FIXTURES = '/viewer-fixtures';

export default {
  title: 'Shared/RemoteMeshViewer',
  component: RemoteMeshViewer,
  args: { testId: 'story-viewer' },
  decorators: [(Story: ComponentType) => <div style={{ maxWidth: 640 }}><Story /></div>],
};

/** A glTF Binary model. */
export const Glb = { tags: ['viewer'], args: { url: `${FIXTURES}/cube.glb`, format: 'glb' } };

/** An OBJ: a format `<model-viewer>` could not show. Its `mtllib` is not
 *  followed, so it draws in the neutral material. */
export const Obj = { tags: ['viewer'], args: { url: `${FIXTURES}/cube.obj`, format: 'obj' } };

/** A 3MF, stored +Z up and turned upright because the caller says so. */
export const ThreeMf = { tags: ['viewer'], args: { url: `${FIXTURES}/textured.3mf`, format: '3mf', storedFrame: 'z_up' } };

/** The real file, of which only the first 40% ever arrives. */
const stalledAt40: typeof fetch = async (input, init) => {
  const whole = new Uint8Array(await (await fetch(input, init)).arrayBuffer());
  const part = whole.subarray(0, Math.round(whole.length * 0.4));
  const body = new ReadableStream<Uint8Array>({ start: (c) => c.enqueue(part) });
  return new Response(body, { headers: { 'content-length': String(whole.length) } });
};

/** Part-way through the download: the provider's preview image shows, and
 *  the bar says how much has arrived. */
export const Downloading = {
  tags: ['viewer-downloading'],
  args: { url: `${FIXTURES}/cube.glb`, format: 'glb', poster: `${FIXTURES}/preview.png`, fetch: stalledAt40 },
};

/** The server answers 404 (stubbed, so the page logs no failed request):
 *  a download failure, so Retry is offered. */
export const Missing = {
  tags: ['viewer-error', 'reason-load'],
  args: { url: `${FIXTURES}/missing.glb`, format: 'glb', fetch: async () => new Response('Not Found', { status: 404 }) },
};

/** The file arrives but is not a glTF: Retry would fetch the same bytes, so
 *  only the download link is offered. */
export const Corrupt = { tags: ['viewer-error', 'reason-parse'], args: { url: `${FIXTURES}/corrupt.glb`, format: 'glb' } };

/** A format the viewer has no loader for: never downloaded. */
export const UnknownFormat = { tags: ['viewer-error', 'reason-format'], args: { url: `${FIXTURES}/cube.xyz`, format: 'xyz' } };

/** 24 viewers down a page, as Generations rows or compare tiles can be. Each
 *  draws only near the viewport and gives its WebGL context back when
 *  scrolled away, so the page never holds more than a browser keeps. */
export const ManyViewers = {
  tags: ['viewer-many'],
  render: () => (
    <div style={{ display: 'grid', gap: 16 }}>
      {Array.from({ length: 24 }, (_, i) => (
        <RemoteMeshViewer key={i} url={`${FIXTURES}/cube.glb`} format="glb" mountWhenVisible testId={`many-${i}`} style={{ height: 220 }} />
      ))}
    </div>
  ),
};
