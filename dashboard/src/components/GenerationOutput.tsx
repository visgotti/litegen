import { useState, type CSSProperties } from 'react';
import type { Generation } from '@litegen/sdk';
import ModelPreview from './ModelPreview';
import { assetsOf } from './model3d-assets';
import { videoResultIsImage } from './generation-output';

const MUTED: CSSProperties = { color: '#8b949e', fontSize: 13 };
const MEDIA: CSSProperties = { maxWidth: '100%', borderRadius: 6, border: '1px solid #30363d' };
const NOTICE: CSSProperties = {
  border: '1px solid #30363d',
  borderRadius: 6,
  padding: 12,
  fontSize: 13,
};
const ERROR_NOTICE: CSSProperties = {
  ...NOTICE,
  background: '#2d1616',
  borderColor: '#f85149',
  color: '#f85149',
};

function clampPercent(n: unknown): number {
  return typeof n === 'number' && Number.isFinite(n) ? Math.min(100, Math.max(0, Math.round(n))) : 0;
}

function InFlight({ generation }: { generation: Generation }) {
  const pct = clampPercent(generation.progress);
  const label = generation.status === 'pending' ? 'Pending' : 'Processing';
  return (
    <div data-testid={`generation-output-${generation.status}`} style={NOTICE}>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 8, color: '#e1e4e8' }}>
        <span>{label}…</span>
        <span style={{ color: '#8b949e' }}>{pct}%</span>
      </div>
      <div
        role="progressbar"
        aria-label={`Generation ${label.toLowerCase()}`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        style={{ height: 6, background: '#21262d', borderRadius: 3, overflow: 'hidden' }}
      >
        <div style={{ width: `${pct}%`, height: '100%', background: '#58a6ff', transition: 'width 300ms ease' }} />
      </div>
    </div>
  );
}

/**
 * A video result. Starts as `<video>` unless the result is known to be an
 * image; if the browser cannot decode it as video, falls back to `<img>` once.
 * That fallback covers GIF "videos" served from extensionless URLs (the mock
 * provider's `/mock/video/{id}`), whose mime the generation row does not keep.
 * Keyed by URL at the call site, so a new result starts over as video.
 */
function VideoResult({ url }: { url: string }) {
  const [asImage, setAsImage] = useState(() => videoResultIsImage(url));
  if (asImage) {
    return <img data-testid="generation-output-image" src={url} alt="Generated output" style={MEDIA} />;
  }
  return (
    <video
      data-testid="generation-output-video"
      controls
      src={url}
      onError={() => setAsImage(true)}
      style={MEDIA}
    />
  );
}

function Completed({ generation }: { generation: Generation }) {
  if (generation.media_type === 'model3d') {
    // Checked before the result_url guard: 3D media lives in metadata.assets,
    // and a completed row with no mesh must reach ModelPreview's own
    // contract-violation state rather than a generic "no output".
    return (
      <div data-testid="generation-output-completed">
        <ModelPreview assets={assetsOf(generation)} testId="generation-output-3d" />
      </div>
    );
  }
  const url = generation.result_url;
  return (
    <div data-testid="generation-output-completed">
      {!url ? (
        <div style={MUTED}>The generation completed but recorded no result URL.</div>
      ) : generation.media_type === 'video' ? (
        <VideoResult key={url} url={url} />
      ) : (
        <img data-testid="generation-output-image" src={url} alt="Generated output" style={MEDIA} />
      )}
    </div>
  );
}

/**
 * Renders one generation row by status. Presentational: the caller fetches
 * (and re-polls) the row and passes the latest state in.
 *
 * `generation` null with no `error` is the "resolving" state — the first fetch
 * is in flight (`loading`), or the row does not exist yet and the caller is
 * still retrying.
 */
export default function GenerationOutput({
  generation,
  loading,
  error,
}: {
  generation: Generation | null;
  loading: boolean;
  error?: string;
}) {
  if (error) {
    return (
      <div data-testid="generation-output-error" style={ERROR_NOTICE}>
        <strong>Could not load the generation:</strong> {error}
      </div>
    );
  }

  if (!generation) {
    return (
      <div data-testid="generation-output-resolving" style={MUTED}>
        {loading ? 'Resolving generation…' : 'Waiting for the generation record…'}
      </div>
    );
  }

  // Widened to string: the wire value is not type-checked, so the default
  // branch must be reachable (switching on the union would narrow it to never).
  const status: string = generation.status;
  switch (status) {
    case 'pending':
    case 'processing':
      return <InFlight generation={generation} />;
    case 'failed':
      return (
        <div data-testid="generation-output-failed" style={ERROR_NOTICE}>
          <strong>Generation failed:</strong> {generation.error_message || 'No reason was recorded.'}
        </div>
      );
    case 'cancelled':
      return (
        <div data-testid="generation-output-cancelled" style={{ ...NOTICE, color: '#e3b341', borderColor: '#e3b341' }}>
          This generation was cancelled.
          {generation.error_message && <div style={{ marginTop: 6, color: '#8b949e' }}>{generation.error_message}</div>}
        </div>
      );
    case 'completed':
      return <Completed generation={generation} />;
    default:
      // An unknown status must say so rather than render nothing.
      return (
        <div data-testid="generation-output-error" style={ERROR_NOTICE}>
          <strong>Unrecognised generation status:</strong> {status}
        </div>
      );
  }
}
