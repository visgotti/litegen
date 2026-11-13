// The compact 3D viewer: the Playground's compare tiles. It is the shared
// viewer (@litegen/3d-viewer, packages/3d-viewer), the same one the 3D
// converter tool draws with, for every format litegen delivers. Loaded on
// first use, so a session that never shows a 3D result never downloads
// three.js. `ModelPreview` is the full inspector around the same viewer.
import { lazy, Suspense, type CSSProperties } from 'react';
import type { ViewerBackground, ViewerLoadInfo } from '@litegen/3d-viewer';
import { fetchStoredFile, useFileSrc } from '../file-src';
import ViewerBoundary from './ViewerBoundary';

const RemoteMeshViewer = lazy(() => import('@litegen/3d-viewer').then((m) => ({ default: m.RemoteMeshViewer })));

export interface ModelViewerProps {
  src: string;
  poster?: string | null;
  /** The mesh's container, from its asset. */
  format?: string;
  testId?: string;
  style?: CSSProperties;
  autoRotate?: boolean;
  exposure?: number;
  background?: ViewerBackground;
  onLoad?: (info: ViewerLoadInfo) => void;
  onError?: (message: string) => void;
}

export default function ModelViewer({ src, poster, format, testId, ...rest }: ModelViewerProps) {
  // The poster is an <img>: a private app's file route reaches it as a
  // blob: URL fetched with the dashboard's credentials. The mesh is fetched
  // by the viewer itself, through the same client (fetchStoredFile).
  const posterSrc = useFileSrc(poster);
  const loading = <span data-testid={testId ? `${testId}-loading` : undefined} style={{ color: '#8b949e' }}>loading viewer…</span>;
  return (
    // A viewer that can't load (its chunk gone after a deploy) leaves the
    // mesh downloadable and the dashboard up.
    <ViewerBoundary key={src} meshUrl={src} testId={testId}>
      <Suspense fallback={loading}>
        <RemoteMeshViewer
          {...rest}
          url={src}
          // An unlabelled mesh (a row older than assets[], whose result_url has
          // no extension) is tried as glTF Binary, as <model-viewer> tried it;
          // a file that isn't one ends in the parse fallback with its link.
          format={format?.toLowerCase() || 'glb'}
          poster={posterSrc ?? null}
          fetch={fetchStoredFile}
          testId={testId}
          // Compare tiles can fill a page: each draws only while on screen.
          mountWhenVisible
        />
      </Suspense>
    </ViewerBoundary>
  );
}
