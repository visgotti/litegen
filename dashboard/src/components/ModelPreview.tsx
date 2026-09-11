import { useEffect, useRef, useState } from 'react';
import type { Model3dAsset } from '@litegen/sdk';
import {
  Check, Copy, Download, ExternalLink, LocateFixed, Maximize2, Minimize2, RotateCw, Sun, TriangleAlert,
} from 'lucide-react';
import ModelViewer, { type ModelViewerHandle, type ViewerBackground } from './ModelViewer';
import {
  copyToClipboard,
  formatBytes,
  formatDimensions,
  isBrowserImage,
  meshOf,
  previewOf,
  texturesOf,
  type Dimensions,
} from './model3d-assets';
import './ModelPreview.css';

const BACKGROUND_OPTIONS: { value: ViewerBackground; label: string }[] = [
  { value: 'dark', label: 'Dark' },
  { value: 'light', label: 'Light' },
  { value: 'checker', label: 'Checker' },
];

const EXPOSURE_MIN = 0.25;
const EXPOSURE_MAX = 2;

interface ViewerState {
  url: string;
  status: 'loading' | 'loaded' | 'error';
  dimensions: Dimensions | null;
}

type CopyState = 'idle' | 'copied' | 'failed';

function upper(format: string): string {
  return format ? format.toUpperCase() : '—';
}

/** Kind-appropriate secondary figure for the asset list. */
function assetDetail(a: Model3dAsset): string {
  if (a.polycount != null) return `${a.polycount.toLocaleString()} polys`;
  if (a.width != null && a.height != null) return `${a.width} × ${a.height}`;
  return '—';
}

function AssetList({ assets, testId }: { assets: Model3dAsset[]; testId: string }) {
  return (
    <>
      <h4 className="mp-section-title" id={`${testId}-assets-title`}>Assets ({assets.length})</h4>
      <div className="mp-assets-wrap">
        <table className="mp-assets" data-testid={`${testId}-assets`} aria-labelledby={`${testId}-assets-title`}>
          <thead>
            <tr>
              <th scope="col">Kind</th>
              <th scope="col">Format</th>
              <th scope="col">Size</th>
              <th scope="col">Detail</th>
              <th scope="col"><span className="mp-sr">Links</span></th>
            </tr>
          </thead>
          <tbody>
            {assets.map((a, i) => (
              <tr key={`${i}:${a.url}`} data-testid={`${testId}-asset-${i}`}>
                <td><span className={`mp-kind mp-kind--${a.kind}`}>{a.kind}</span></td>
                <td>{upper(a.format)}</td>
                <td className="mp-num">{formatBytes(a.size_bytes)}</td>
                <td className="mp-num">{assetDetail(a)}</td>
                <td className="mp-links">
                  {/* Asset URLs are absolute and usually cross-origin, where
                      browsers ignore `download`; target=_blank keeps that case
                      from navigating the dashboard away. */}
                  <a
                    href={a.url}
                    target="_blank"
                    rel="noreferrer"
                    data-testid={`${testId}-asset-${i}-open`}
                    aria-label={`Open ${a.kind} ${upper(a.format)} in a new tab`}
                  >
                    <ExternalLink size={12} aria-hidden="true" />Open
                  </a>
                  <a
                    href={a.url}
                    download
                    target="_blank"
                    rel="noreferrer"
                    data-testid={`${testId}-asset-${i}-download`}
                    aria-label={`Download ${a.kind} ${upper(a.format)}`}
                  >
                    <Download size={12} aria-hidden="true" />Download
                  </a>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

/**
 * The full 3D inspector: an interactive viewer plus toolbar, stats, thumbnails
 * and the complete asset list. Used where one result has the user's attention
 * (Generations expanded row, Playground single-mode result). Grid tiles and
 * row thumbnails deliberately stay on the compact `ModelViewer` / flat image:
 * a GL canvas with this much chrome per tile would be heavy and noisy.
 *
 * `testId` prefixes every `data-testid` inside so several inspectors can share
 * a page (multiple expanded Generations rows).
 */
export default function ModelPreview({
  assets,
  error,
  testId = 'model-preview',
}: {
  assets: Model3dAsset[];
  /** A failure the caller already knows about; replaces the viewer. */
  error?: string;
  testId?: string;
}) {
  const rootRef = useRef<HTMLElement | null>(null);
  const viewerRef = useRef<ModelViewerHandle | null>(null);
  const copyTimer = useRef<number | undefined>(undefined);

  const [autoRotate, setAutoRotate] = useState(true);
  const [exposure, setExposure] = useState(1);
  const [background, setBackground] = useState<ViewerBackground>('dark');
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [copyState, setCopyState] = useState<CopyState>('idle');
  const [viewer, setViewer] = useState<ViewerState>({ url: '', status: 'loading', dimensions: null });

  const mesh = meshOf(assets);
  const preview = previewOf(assets);
  const thumbs = [preview, ...texturesOf(assets)].filter(
    (a): a is Model3dAsset => a != null && isBrowserImage(a.format),
  );
  // Keyed by mesh URL and derived during render, so switching results never
  // shows the previous mesh's dimensions or failure.
  const view: ViewerState = mesh && viewer.url === mesh.url
    ? viewer
    : { url: mesh?.url ?? '', status: 'loading', dimensions: null };
  const viewerFailed = view.status === 'error';

  // Hidden entirely (not disabled) where the Fullscreen API is unavailable,
  // e.g. iPhone Safari, since there is nothing the user could do to enable it.
  const canFullscreen = typeof document !== 'undefined' && document.fullscreenEnabled === true;

  useEffect(() => {
    // Mirrors the real fullscreen state, which also changes on Esc or browser
    // UI without going through our button.
    const onChange = () => setIsFullscreen(document.fullscreenElement != null && document.fullscreenElement === rootRef.current);
    document.addEventListener('fullscreenchange', onChange);
    return () => document.removeEventListener('fullscreenchange', onChange);
  }, []);

  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  if (error || !mesh) {
    return (
      <section className="mp" data-testid={testId} data-state="error" aria-label="3D model inspector">
        <div className="mp-error" role="alert" data-testid={`${testId}-error`}>
          <TriangleAlert size={16} aria-hidden="true" />
          <div>
            <strong>{error ? '3D generation failed' : 'No mesh in this result'}</strong>
            <p>
              {error
                ?? 'This generation completed without a mesh asset. Every completed 3D generation must carry exactly one mesh, so this is a provider contract violation — there is nothing to render.'}
            </p>
          </div>
        </div>
        {assets.length > 0 && <AssetList assets={assets} testId={testId} />}
      </section>
    );
  }

  const toggleFullscreen = () => {
    const el = rootRef.current;
    if (!el) return;
    // Both calls reject when the browser refuses (no user gesture, iframe
    // without allowfullscreen); the button simply stays unpressed.
    if (document.fullscreenElement === el) {
      document.exitFullscreen().catch(() => {});
    } else {
      el.requestFullscreen().catch(() => {});
    }
  };

  const copyMeshUrl = async () => {
    const ok = await copyToClipboard(mesh.url, typeof navigator === 'undefined' ? undefined : navigator.clipboard);
    setCopyState(ok ? 'copied' : 'failed');
    window.clearTimeout(copyTimer.current);
    copyTimer.current = window.setTimeout(() => setCopyState('idle'), 2000);
  };

  const meshUrl = mesh.url;
  // Buttons with visible text take it as their accessible name (no aria-label
  // to drift out of sync with it); only the icon-only fullscreen button and
  // the background swatches, which need the word "background", carry one.
  const copyLabel = copyState === 'copied' ? 'Copied' : copyState === 'failed' ? 'Copy failed' : 'Copy mesh URL';

  return (
    <section
      ref={rootRef}
      className="mp"
      data-testid={testId}
      data-state={view.status}
      data-fullscreen={isFullscreen ? 'true' : undefined}
      aria-label="3D model inspector"
    >
      <div className="mp-toolbar" role="group" aria-label="Viewer controls">
        <button
          type="button"
          className="btn btn-secondary mp-btn"
          aria-pressed={autoRotate}
          data-testid={`${testId}-autorotate`}
          disabled={viewerFailed}
          onClick={() => setAutoRotate(v => !v)}
        >
          <RotateCw size={14} aria-hidden="true" />Auto-rotate
        </button>
        <button
          type="button"
          className="btn btn-secondary mp-btn"
          data-testid={`${testId}-reset`}
          disabled={viewerFailed}
          onClick={() => viewerRef.current?.resetCamera()}
        >
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
            disabled={viewerFailed}
            aria-valuetext={exposure.toFixed(2)}
            data-testid={`${testId}-exposure`}
            onChange={e => setExposure(Number(e.target.value))}
          />
          <output aria-hidden="true">{exposure.toFixed(2)}</output>
        </label>
        <div className="mp-seg" role="group" aria-label="Background">
          {BACKGROUND_OPTIONS.map(o => (
            <button
              key={o.value}
              type="button"
              className="btn btn-secondary mp-btn"
              aria-pressed={background === o.value}
              aria-label={`${o.label} background`}
              data-testid={`${testId}-bg-${o.value}`}
              disabled={viewerFailed}
              onClick={() => setBackground(o.value)}
            >
              <span className={`mp-swatch mp-swatch--${o.value}`} aria-hidden="true" />
              {o.label}
            </button>
          ))}
        </div>
        <span className="mp-spacer" />
        {canFullscreen && (
          <button
            type="button"
            className="btn btn-secondary mp-btn"
            aria-pressed={isFullscreen}
            aria-label={isFullscreen ? 'Exit fullscreen' : 'Enter fullscreen'}
            title={isFullscreen ? 'Exit fullscreen' : 'Fullscreen'}
            data-testid={`${testId}-fullscreen`}
            disabled={viewerFailed}
            onClick={toggleFullscreen}
          >
            {isFullscreen ? <Minimize2 size={14} aria-hidden="true" /> : <Maximize2 size={14} aria-hidden="true" />}
          </button>
        )}
        <button
          type="button"
          className="btn btn-secondary mp-btn"
          title={meshUrl}
          data-testid={`${testId}-copy-url`}
          data-copy-state={copyState}
          onClick={copyMeshUrl}
        >
          {copyState === 'copied'
            ? <Check size={14} aria-hidden="true" />
            : <Copy size={14} aria-hidden="true" />}
          {copyLabel}
        </button>
        <span className="mp-sr" role="status" aria-live="polite">
          {copyState === 'copied' ? 'Mesh URL copied' : copyState === 'failed' ? 'Could not copy the mesh URL' : ''}
        </span>
      </div>

      <div className="mp-stage">
        <ModelViewer
          ref={viewerRef}
          src={meshUrl}
          poster={preview?.url}
          autoRotate={autoRotate}
          exposure={exposure}
          background={background}
          testId={`${testId}-viewer`}
          style={{ height: isFullscreen ? '100%' : 420, borderRadius: 0 }}
          onLoad={({ dimensions }) => setViewer({ url: meshUrl, status: 'loaded', dimensions })}
          onError={() => setViewer({ url: meshUrl, status: 'error', dimensions: null })}
        />
      </div>

      <dl className="mp-stats" data-testid={`${testId}-stats`}>
        <div data-testid={`${testId}-stat-format`}>
          <dt>Format</dt>
          <dd>{upper(mesh.format)}</dd>
        </div>
        <div data-testid={`${testId}-stat-size`}>
          <dt>Size</dt>
          <dd>{formatBytes(mesh.size_bytes)}</dd>
        </div>
        <div data-testid={`${testId}-stat-polycount`}>
          <dt>Polycount</dt>
          <dd>{mesh.polycount != null ? mesh.polycount.toLocaleString() : '—'}</dd>
        </div>
        <div data-testid={`${testId}-stat-dimensions`}>
          <dt>Dimensions</dt>
          <dd>
            {view.status === 'loading'
              ? <span className="mp-pending">measuring…</span>
              : formatDimensions(view.dimensions)}
          </dd>
        </div>
      </dl>

      {thumbs.length > 0 && (
        <ul className="mp-thumbs" data-testid={`${testId}-thumbs`} aria-label="Preview and textures">
          {thumbs.map((a, i) => (
            <li key={`${i}:${a.url}`}>
              <a href={a.url} target="_blank" rel="noreferrer" data-testid={`${testId}-thumb-${i}`}>
                <img src={a.url} alt={`${a.kind} (${upper(a.format)})`} loading="lazy" />
                <span aria-hidden="true">{a.kind}</span>
              </a>
            </li>
          ))}
        </ul>
      )}

      <AssetList assets={assets} testId={testId} />
    </section>
  );
}
