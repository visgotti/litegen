import React, { useEffect, useEffectEvent, useImperativeHandle, useRef, useState } from 'react';
// Type-only: erased at compile time, so it does not defeat the lazy import
// below, but it makes tsc check every element API used here (getDimensions,
// resetTurntableRotation, cameraOrbit, …) against the installed version.
import type { ModelViewerElement } from '@google/model-viewer';
import { describeViewerError, type Dimensions } from './model3d-assets';

export type ViewerBackground = 'dark' | 'light' | 'checker';

/** Camera reset is a one-shot command rather than state, so it is exposed as
 *  an imperative handle instead of a prop the caller would have to toggle. */
export interface ModelViewerHandle {
  resetCamera(): void;
}

export interface ModelViewerLoadInfo {
  /** Bounding-box size in metres; null if the element could not report it. */
  dimensions: Dimensions | null;
}

const BACKGROUNDS: Record<ViewerBackground, string> = {
  dark: '#0d1117',
  light: '#f0f3f6',
  // Transparency checkerboard, kept dark so it sits in the dashboard palette.
  checker: 'repeating-conic-gradient(#30363d 0% 25%, #161b22 0% 50%) 50% / 20px 20px',
};

// The framing is set explicitly (these equal model-viewer's own defaults) so
// that "reset camera" re-applies values this file owns rather than relying on
// an undocumented default staying put across upgrades.
const CAMERA_ORBIT = '0deg 75deg 105%';
const CAMERA_TARGET = 'auto auto auto';
const FIELD_OF_VIEW = 'auto';

interface LoadState {
  src: string;
  status: 'loading' | 'loaded' | 'error';
  progress: number;
  message: string;
}

const loadingState = (src: string): LoadState => ({ src, status: 'loading', progress: 0, message: '' });

/** The one failure presentation, shared by "the viewer bundle failed to
 *  import" and "the element failed to load this mesh": the mesh is still
 *  downloadable, so a failure degrades to a link rather than an empty box. */
function MeshFallback({ src, message, testId }: { src: string; message: string; testId?: string }) {
  return (
    <div
      role="alert"
      data-testid={testId ? `${testId}-fallback` : undefined}
      style={{
        display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', gap: 8,
        minHeight: 160, padding: 16, textAlign: 'center', boxSizing: 'border-box', width: '100%',
        background: '#0d1117', border: '1px dashed #30363d', borderRadius: 6, color: '#8b949e', fontSize: 13,
      }}
    >
      <span>{message}</span>
      <a href={src} download data-testid={testId ? `${testId}-fallback-link` : undefined} style={{ color: '#58a6ff' }}>
        Download mesh
      </a>
    </div>
  );
}

/**
 * Lazily loads the `<model-viewer>` custom element the first time a 3D result is
 * actually shown, so the ~1MB WebGL bundle never lands on a dashboard session
 * that only looks at images.
 *
 * This is the compact viewer (grid tiles, trace panel). `ModelPreview` wraps
 * it with the inspector chrome; the display props below exist for that.
 */
export default function ModelViewer({
  src,
  poster,
  testId,
  style,
  autoRotate = true,
  exposure = 1,
  background = 'dark',
  onLoad,
  onError,
  ref,
}: {
  src: string;
  poster?: string | null;
  testId?: string;
  style?: React.CSSProperties;
  autoRotate?: boolean;
  exposure?: number;
  background?: ViewerBackground;
  onLoad?: (info: ModelViewerLoadInfo) => void;
  onError?: (message: string) => void;
  ref?: React.Ref<ModelViewerHandle>;
}) {
  const [ready, setReady] = useState(() => !!customElements.get('model-viewer'));
  const [failed, setFailed] = useState(false);
  const elRef = useRef<ModelViewerElement | null>(null);
  // Load state is keyed by src and derived during render, so a new src reads
  // as "loading" immediately without a reset effect.
  const [load, setLoad] = useState<LoadState>(() => loadingState(src));
  const current = load.src === src ? load : loadingState(src);
  const showElement = ready && current.status !== 'error';

  const handleLoad = useEffectEvent((el: ModelViewerElement) => {
    let dimensions: Dimensions | null = null;
    try {
      const d = el.getDimensions();
      dimensions = { x: d.x, y: d.y, z: d.z };
    } catch {
      // A scene that reports no bounds leaves the stat blank; not a load failure.
    }
    setLoad({ src, status: 'loaded', progress: 1, message: '' });
    onLoad?.({ dimensions });
  });

  const handleError = useEffectEvent((detail: unknown) => {
    const message = describeViewerError(detail);
    setLoad({ src, status: 'error', progress: 0, message });
    onError?.(message);
  });

  const handleProgress = useEffectEvent((progress: number) => {
    // `progress` keeps firing around completion; never regress a settled state.
    setLoad(prev => (prev.src === src && prev.status !== 'loading'
      ? prev
      : { src, status: 'loading', progress, message: '' }));
  });

  // Listeners go on the element directly via the ref: these are CustomEvents
  // from a web component, and `load`/`error` collide with names React special-
  // cases for <img>/<video>, so JSX `on*` props are not a reliable route.
  useEffect(() => {
    const el = elRef.current;
    if (!showElement || !el) return;
    const onLoadEvent = () => handleLoad(el);
    const onErrorEvent = (e: Event) => handleError((e as CustomEvent).detail);
    const onProgressEvent = (e: Event) =>
      handleProgress(Number((e as CustomEvent<{ totalProgress?: number }>).detail?.totalProgress) || 0);
    el.addEventListener('load', onLoadEvent);
    el.addEventListener('error', onErrorEvent);
    el.addEventListener('progress', onProgressEvent);
    // The element is fresh here, so `loaded` can only mean this mesh finished
    // before the effect ran (e.g. a cached file) — don't miss that load.
    if (el.loaded) onLoadEvent();
    return () => {
      el.removeEventListener('load', onLoadEvent);
      el.removeEventListener('error', onErrorEvent);
      el.removeEventListener('progress', onProgressEvent);
    };
  }, [showElement]);

  useImperativeHandle(ref, () => ({
    resetCamera() {
      const el = elRef.current;
      if (!el) return;
      el.resetTurntableRotation(0);
      // model-viewer 4.x declares these three with `hasChanged: () => true`,
      // so re-assigning the same value re-syncs the camera even though user
      // orbiting never touched the properties. Left to animate to the goal.
      el.cameraTarget = CAMERA_TARGET;
      el.fieldOfView = FIELD_OF_VIEW;
      el.cameraOrbit = CAMERA_ORBIT;
    },
  }), []);

  useEffect(() => {
    if (ready) return;
    let cancelled = false;
    import('@google/model-viewer')
      .then(() => { if (!cancelled) setReady(true); })
      .catch(() => { if (!cancelled) setFailed(true); });
    return () => { cancelled = true; };
  }, [ready]);

  if (failed) {
    return <MeshFallback src={src} testId={testId} message="The 3D viewer could not be loaded in this browser." />;
  }
  if (current.status === 'error') {
    return <MeshFallback src={src} testId={testId} message={current.message} />;
  }
  if (!ready) {
    return <span data-testid={testId ? `${testId}-loading` : undefined} style={{ color: '#8b949e' }}>loading viewer…</span>;
  }

  const pct = Math.round(current.progress * 100);
  return (
    <div className="model-viewer-wrap" style={{ position: 'relative', width: '100%' }}>
      {React.createElement(
        'model-viewer',
        {
          ref: elRef,
          src,
          poster: poster ?? undefined,
          alt: '3D model preview',
          'camera-controls': true,
          // Boolean attributes are omitted rather than set to false: a present
          // `auto-rotate="false"` attribute would still read as on.
          'auto-rotate': autoRotate || undefined,
          'camera-orbit': CAMERA_ORBIT,
          'camera-target': CAMERA_TARGET,
          'field-of-view': FIELD_OF_VIEW,
          // A number, not a string: React 19 assigns this as the element's
          // `exposure` property (it exists on the upgraded element), and the
          // renderer uses the property value as-is.
          exposure,
          'shadow-intensity': '1',
          // Lets a vertical swipe scroll the page on touch screens instead of
          // being captured as an orbit (the viewer sits inside scrolling tables).
          'touch-action': 'pan-y',
          'data-testid': testId,
          style: { width: '100%', height: 360, background: BACKGROUNDS[background], borderRadius: 6, display: 'block', ...style },
        },
        // An empty slot replaces model-viewer's built-in progress bar with ours.
        React.createElement('div', { slot: 'progress-bar' }),
      )}
      {current.status === 'loading' && (
        <div
          role="progressbar"
          aria-label="Loading mesh"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={pct}
          data-testid={testId ? `${testId}-progress` : undefined}
          style={{ position: 'absolute', top: 0, left: 0, right: 0, height: 3, background: '#21262d', borderRadius: '6px 6px 0 0', overflow: 'hidden', pointerEvents: 'none' }}
        >
          <div style={{ width: `${Math.max(pct, 2)}%`, height: '100%', background: '#58a6ff', transition: 'width 120ms linear' }} />
        </div>
      )}
    </div>
  );
}
