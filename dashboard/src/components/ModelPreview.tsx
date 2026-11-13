// The full 3D inspector: an interactive viewer with its toolbar, stats,
// scene facts, clips, thumbnails and the complete asset list. Used where one
// result has the user's attention (Generations expanded row, Playground
// single-mode result, Logs trace panel). Compare tiles use the compact
// `ModelViewer`. Both are the shared viewer (@litegen/3d-viewer) that the 3D
// converter tool shows too.
//
// The inspector itself (./ModelPreviewBody) loads on first use, so a session
// that never shows a 3D result never downloads three.js. If it can't be
// loaded (a chunk gone after a deploy), the mesh stays downloadable and the
// rest of the dashboard stays up (ViewerBoundary).
//
// `testId` prefixes every `data-testid` inside so several inspectors can share
// a page (multiple expanded Generations rows).
import { lazy, Suspense } from 'react';
import type { ModelPreviewProps } from './ModelPreviewBody';
import { meshOf, validAssets } from './model3d-assets';
import ViewerBoundary from './ViewerBoundary';

const ModelPreviewBody = lazy(() => import('./ModelPreviewBody'));

export default function ModelPreview(props: ModelPreviewProps) {
  const testId = props.testId ?? 'model-preview';
  const meshUrl = meshOf(validAssets(props.assets))?.url;
  return (
    <ViewerBoundary key={meshUrl} meshUrl={meshUrl} testId={`${testId}-viewer`}>
      <Suspense
        fallback={
          <section className="mp" data-testid={testId} data-state="loading" aria-label="3D model inspector">
            <span data-testid={`${testId}-loading`} style={{ color: '#8b949e' }}>loading viewer…</span>
          </section>
        }
      >
        <ModelPreviewBody {...props} testId={testId} />
      </Suspense>
    </ViewerBoundary>
  );
}
