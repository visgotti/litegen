// The 3D inspector: a viewer with a toolbar (auto-rotate, animation, reset
// camera, exposure, background, fullscreen and the caller's actions), its
// stats (format, size, triangles, dimensions), a note when the format's
// loader drew less than litegen reads, then the caller's details below.
//
// Formerly two copies: the dashboard's ModelPreview around <model-viewer>,
// and the converter tool's Inspector, ported from it, around three.js.
// Both apps now render this, each through its own adapter.
//
// `testId` prefixes every test ID inside, so several inspectors can share a
// page (two panes in the tool, expanded Generations rows in the dashboard).
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { LocateFixed, Maximize2, Minimize2, Pause, Play, RotateCw, Sun } from 'lucide-react';
import MeshViewer, { type MeshViewerHandle, type ViewerBackground, type ViewerLoadInfo } from '../viewer/MeshViewer';
import RemoteMeshViewer from '../viewer/RemoteMeshViewer';
import { drawnNote, formatBytes, formatDimensions, type Dimensions } from '../format';
import type { StoredFrame, UpAxis, ViewerFile } from '../types';
import './inspector.css';

const BACKGROUNDS: { value: ViewerBackground; label: string }[] = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
  { value: 'checker', label: 'Checker' },
];

const EXPOSURE_MIN = 0.25;
const EXPOSURE_MAX = 2;

/** The model's bytes in memory (the converter tool), or a URL to download
 *  it from (the dashboard), with the provider's 2D render to show until
 *  it's drawn. */
export type InspectorSource =
  | { files: ViewerFile[] }
  | { url: string; poster?: string | null; fetch?: typeof globalThis.fetch };

export interface ModelInspectorProps {
  source: InspectorSource;
  /** The container, lowercase: `glb`, `obj`… */
  format: string;
  /** The format's human name, e.g. "glTF Binary", where known: the Format
   *  stat's tooltip. */
  formatLabel?: string;
  storedFrame?: StoredFrame;
  up?: UpAxis;
  /** Bytes of the model's files. */
  size?: number | null;
  /** Triangles litegen reads in the file: the converter's own reading, or
   *  the API's scene manifest. */
  triangles?: number | null;
  /** The model's size in metres when litegen measured it. Absent, the size
   *  the viewer drew it at, which three.js's loaders do not all give in
   *  metres. */
  dimensions?: Dimensions | null;
  /** In the toolbar, e.g. "Input". */
  title?: string;
  /** Buttons at the end of the toolbar, e.g. copy URL. */
  actions?: ReactNode;
  /** Draw only near the viewport (see MeshViewer). */
  mountWhenVisible?: boolean;
  testId: string;
  /** Below the stats: facts, tables, notes, thumbnails. */
  children?: ReactNode;
}

type Load = { key: unknown; status: 'loading' | 'loaded' | 'error'; dimensions: Dimensions | null; clips: number; triangles?: number };

export default function ModelInspector({
  source,
  format,
  formatLabel,
  storedFrame,
  up,
  size,
  triangles,
  dimensions,
  title,
  actions,
  mountWhenVisible,
  testId,
  children,
}: ModelInspectorProps) {
  const rootRef = useRef<HTMLElement | null>(null);
  const viewerRef = useRef<MeshViewerHandle | null>(null);
  const [autoRotate, setAutoRotate] = useState(true);
  const [animate, setAnimate] = useState(true);
  const [exposure, setExposure] = useState(1);
  const [background, setBackground] = useState<ViewerBackground>('dark');
  const [isFullscreen, setIsFullscreen] = useState(false);
  // Keyed by what is shown, so a new model never shows the last one's size
  // or failure.
  const key = 'files' in source ? source.files : source.url;
  const [load, setLoad] = useState<Load>({ key, status: 'loading', dimensions: null, clips: 0 });
  const current: Load = load.key === key ? load : { key, status: 'loading', dimensions: null, clips: 0 };
  // Viewer-only controls act on nothing without a drawn model; the caller's
  // actions (copy URL, downloads) stay, since that is when they matter.
  const failed = current.status === 'error';
  // Hidden, not disabled, where the Fullscreen API is missing (iPhone
  // Safari): there is nothing the user could do to enable it.
  const canFullscreen = typeof document !== 'undefined' && document.fullscreenEnabled === true;

  useEffect(() => {
    // The real fullscreen state, which also changes on Esc or browser UI.
    const onChange = () => setIsFullscreen(document.fullscreenElement != null && document.fullscreenElement === rootRef.current);
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  const toggleFullscreen = () => {
    const el = rootRef.current;
    if (!el) return;
    // Both reject when the browser refuses (no user gesture, an iframe
    // without allowfullscreen); the button simply stays unpressed.
    if (document.fullscreenElement === el) document.exitFullscreen().catch(() => {});
    else el.requestFullscreen().catch(() => {});
  };

  const viewer = {
    ref: viewerRef,
    format,
    storedFrame,
    up,
    autoRotate,
    animate,
    exposure,
    background,
    mountWhenVisible,
    testId: `${testId}-viewer`,
    style: { height: isFullscreen ? '100%' : 420, borderRadius: 0 },
    onLoad: ({ dimensions: drawn, clips, triangles: drawnTriangles }: ViewerLoadInfo) =>
      setLoad({ key, status: 'loaded', dimensions: drawn, clips, triangles: drawnTriangles }),
    onError: () => setLoad({ key, status: 'error', dimensions: null, clips: 0 }),
  };
  const note = current.status === 'loaded' ? drawnNote(triangles, current.triangles) : null;

  return (
    <section
      ref={rootRef}
      className="mp"
      data-testid={testId}
      data-state={current.status}
      data-fullscreen={isFullscreen ? 'true' : undefined}
      aria-label={title ? `${title} model` : '3D model inspector'}
    >
      <div className="mp-toolbar" role="group" aria-label="Viewer controls">
        {title && <h3 className="mp-section-title mp-toolbar-title">{title}</h3>}
        <div className="mp-toolbar-group">
          <button type="button" className="btn btn-secondary mp-btn" aria-pressed={autoRotate} disabled={failed} onClick={() => setAutoRotate((v) => !v)} data-testid={`${testId}-autorotate`}>
            <RotateCw size={14} aria-hidden="true" />Auto-rotate
          </button>
          {current.clips > 0 && (
            <button type="button" className="btn btn-secondary mp-btn" aria-pressed={animate} disabled={failed} onClick={() => setAnimate((v) => !v)} data-testid={`${testId}-animate`}>
              {animate ? <Pause size={14} aria-hidden="true" /> : <Play size={14} aria-hidden="true" />}Animation
            </button>
          )}
          <button type="button" className="btn btn-secondary mp-btn" disabled={failed} onClick={() => viewerRef.current?.resetCamera()} data-testid={`${testId}-reset`}>
            <LocateFixed size={14} aria-hidden="true" />Reset camera
          </button>
          <label className="mp-exposure">
            <Sun size={14} aria-hidden="true" />
            <span>Exposure</span>
            <input
              type="range"
              min={EXPOSURE_MIN}
              max={EXPOSURE_MAX}
              step={0.05}
              value={exposure}
              disabled={failed}
              aria-valuetext={exposure.toFixed(2)}
              data-testid={`${testId}-exposure`}
              onChange={(e) => setExposure(Number(e.target.value))}
            />
            <output aria-hidden="true">{exposure.toFixed(2)}</output>
          </label>
        </div>
        <div className="mp-toolbar-group">
          <div className="mp-seg" role="group" aria-label="Background">
            {BACKGROUNDS.map((o) => (
              <button
                key={o.value}
                type="button"
                className="btn btn-secondary mp-btn"
                aria-pressed={background === o.value}
                aria-label={`${o.label} background`}
                data-testid={`${testId}-bg-${o.value}`}
                disabled={failed}
                onClick={() => setBackground(o.value)}
              >
                <span className={`mp-swatch mp-swatch--${o.value}`} aria-hidden="true" />
                {o.label}
              </button>
            ))}
          </div>
        </div>
        <div className="mp-toolbar-group mp-toolbar-actions">
          {canFullscreen && (
            <button
              type="button"
              className="btn btn-secondary mp-btn"
              aria-pressed={isFullscreen}
              aria-label={isFullscreen ? 'Exit fullscreen' : 'Enter fullscreen'}
              title={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
              data-testid={`${testId}-fullscreen`}
              disabled={failed}
              onClick={toggleFullscreen}
            >
              {isFullscreen ? <Minimize2 size={14} aria-hidden="true" /> : <Maximize2 size={14} aria-hidden="true" />}
            </button>
          )}
          {actions}
        </div>
      </div>

      <div className="mp-stage">
        {'files' in source ? (
          <MeshViewer {...viewer} files={source.files} />
        ) : (
          <RemoteMeshViewer {...viewer} url={source.url} poster={source.poster} fetch={source.fetch} />
        )}
      </div>

      <dl className="mp-stats" data-testid={`${testId}-stats`}>
        <div data-testid={`${testId}-stat-format`}>
          <dt>Format</dt>
          <dd title={formatLabel}>{format ? format.toUpperCase() : '—'}</dd>
        </div>
        <div data-testid={`${testId}-stat-size`}>
          <dt>Size</dt>
          <dd>{formatBytes(size)}</dd>
        </div>
        <div data-testid={`${testId}-stat-triangles`}>
          <dt>Triangles</dt>
          <dd>{triangles != null ? triangles.toLocaleString('en-US') : '—'}</dd>
        </div>
        <div data-testid={`${testId}-stat-dimensions`}>
          <dt>Dimensions</dt>
          <dd>
            {dimensions !== undefined
              ? formatDimensions(dimensions)
              : current.status === 'loading'
                ? <span className="mp-pending">measuring…</span>
                : formatDimensions(current.dimensions)}
          </dd>
        </div>
      </dl>
      {note && (
        <p className="mp-note" role="note" data-testid={`${testId}-drawn-note`}>
          {note}
        </p>
      )}
      <div className="mp-details">{children}</div>
    </section>
  );
}
