import React, { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { RequestLog, RequestArtifact } from '@litegen/sdk';

const TAB_KEY = 'litegen_trace_panel_tab';

type Tab = 'visual' | 'prompt' | 'params' | 'response';

interface TracePanelProps {
  logId: string | null;
  onClose: () => void;
}

async function fetchLog(id: string): Promise<RequestLog> {
  const data = await client.logs.list({ page: 1, per_page: 1000 });
  const found = data.data.find(l => l.id === id);
  if (!found) throw new Error('Log not found');
  return found;
}

export default function TracePanel({ logId, onClose }: TracePanelProps) {
  const [tab, setTab] = useState<Tab>(() => {
    return (localStorage.getItem(TAB_KEY) as Tab) || 'visual';
  });
  const [log, setLog] = useState<RequestLog | null>(null);
  const [artifact, setArtifact] = useState<RequestArtifact | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!logId) {
      setLog(null);
      setArtifact(null);
      setError('');
      return;
    }
    setLoading(true);
    setError('');
    Promise.all([
      fetchLog(logId),
      client.logs.getArtifact(logId).catch(() => null),
    ]).then(([l, a]) => {
      setLog(l);
      setArtifact(a);
    }).catch(e => {
      setError(e.message);
    }).finally(() => setLoading(false));
  }, [logId]);

  const switchTab = (t: Tab) => {
    setTab(t);
    localStorage.setItem(TAB_KEY, t);
  };

  if (!logId) return null;

  const panelStyle: React.CSSProperties = {
    position: 'fixed',
    top: 0,
    right: 0,
    height: '100vh',
    width: 'min(50vw, 720px)',
    minWidth: 320,
    background: '#161b22',
    borderLeft: '1px solid #30363d',
    zIndex: 1000,
    display: 'flex',
    flexDirection: 'column',
    boxShadow: '-4px 0 24px rgba(0,0,0,0.6)',
  };

  const backdropStyle: React.CSSProperties = {
    position: 'fixed',
    inset: 0,
    background: 'rgba(0,0,0,0.5)',
    zIndex: 999,
  };

  const tabs: { key: Tab; label: string }[] = [
    { key: 'visual', label: 'Visual' },
    { key: 'prompt', label: 'Prompt' },
    { key: 'params', label: 'Params' },
    { key: 'response', label: 'Response' },
  ];

  return (
    <>
      <div style={backdropStyle} onClick={onClose} />
      <div data-testid="trace-panel" style={panelStyle}>
        {/* Header */}
        <div style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '12px 16px',
          borderBottom: '1px solid #30363d',
          flexShrink: 0,
        }}>
          <span style={{ color: '#e1e4e8', fontWeight: 600, fontSize: 14 }}>
            Trace: <code style={{ fontSize: 12, color: '#8b949e' }}>{logId}</code>
          </span>
          <button
            data-testid="trace-panel-close"
            onClick={onClose}
            style={{
              background: 'none',
              border: 'none',
              color: '#8b949e',
              cursor: 'pointer',
              fontSize: 18,
              lineHeight: 1,
              padding: '2px 6px',
            }}
            aria-label="Close trace panel"
          >
            ×
          </button>
        </div>

        {/* Tabs */}
        <div style={{
          display: 'flex',
          borderBottom: '1px solid #30363d',
          flexShrink: 0,
        }}>
          {tabs.map(t => (
            <button
              key={t.key}
              data-testid={`trace-tab-${t.key}`}
              onClick={() => switchTab(t.key)}
              style={{
                background: 'none',
                border: 'none',
                borderBottom: tab === t.key ? '2px solid #58a6ff' : '2px solid transparent',
                color: tab === t.key ? '#58a6ff' : '#8b949e',
                cursor: 'pointer',
                padding: '10px 16px',
                fontSize: 13,
                fontWeight: tab === t.key ? 600 : 400,
              }}
            >
              {t.label}
            </button>
          ))}
        </div>

        {/* Body */}
        <div style={{ flex: 1, overflow: 'auto', padding: 16 }}>
          {loading && (
            <div style={{ color: '#8b949e', fontSize: 13 }}>Loading...</div>
          )}
          {error && (
            <div style={{ color: '#f85149', fontSize: 13 }}>{error}</div>
          )}
          {!loading && !error && (
            <>
              {tab === 'visual' && <VisualTab artifact={artifact} />}
              {tab === 'prompt' && <PromptTab artifact={artifact} />}
              {tab === 'params' && <ParamsTab artifact={artifact} log={log} />}
              {tab === 'response' && <ResponseTab log={log} />}
            </>
          )}
        </div>
      </div>
    </>
  );
}

function VisualTab({ artifact }: { artifact: RequestArtifact | null }) {
  if (!artifact) {
    return <div style={{ color: '#8b949e', fontSize: 13 }}>No artifact found for this request.</div>;
  }

  if (artifact.output_kind === 'error') {
    return (
      <div
        data-testid="trace-visual-error"
        style={{
          background: '#2d1616',
          border: '1px solid #f85149',
          borderRadius: 6,
          padding: 12,
          color: '#f85149',
          fontSize: 13,
        }}
      >
        <strong>Error:</strong> {artifact.error_message || 'Unknown error'}
      </div>
    );
  }

  if (artifact.output_kind === 'b64' && artifact.output_value) {
    const mime = artifact.output_mime || 'image/png';
    return (
      <div>
        {artifact.output_truncated && (
          <div style={{
            background: '#161b22',
            border: '1px solid #e3b341',
            borderRadius: 4,
            padding: '4px 8px',
            marginBottom: 8,
            color: '#e3b341',
            fontSize: 12,
          }}>
            Preview only — full output exceeded 2MB cap
          </div>
        )}
        <img
          data-testid="trace-visual-image"
          src={`data:${mime};base64,${artifact.output_value}`}
          alt="Generated output"
          style={{ maxWidth: '100%', borderRadius: 6, border: '1px solid #30363d' }}
        />
      </div>
    );
  }

  if (artifact.output_kind === 'url' && artifact.output_value) {
    // GIF videos (e.g. mock/visual-video-gen) are served as image/gif —
    // render them as <img> so the browser animates them inline.
    const mime = artifact.output_mime ?? '';
    const isGif = mime.startsWith('image/');
    if (artifact.media_type === 'video' && !isGif) {
      return (
        <video
          data-testid="trace-visual-video"
          controls
          src={artifact.output_value}
          style={{ maxWidth: '100%', borderRadius: 6, border: '1px solid #30363d' }}
        />
      );
    }
    return (
      <img
        data-testid="trace-visual-image"
        src={artifact.output_value}
        alt="Generated output"
        style={{ maxWidth: '100%', borderRadius: 6, border: '1px solid #30363d' }}
      />
    );
  }

  if (artifact.output_kind === 'url' && !artifact.output_value) {
    return (
      <div style={{ color: '#8b949e', fontSize: 13 }}>
        Output URL not yet available (async generation pending).
      </div>
    );
  }

  return <div style={{ color: '#8b949e', fontSize: 13 }}>No output available.</div>;
}

function PromptTab({ artifact }: { artifact: RequestArtifact | null }) {
  if (!artifact) {
    return <div style={{ color: '#8b949e', fontSize: 13 }}>No artifact found for this request.</div>;
  }

  let refs: Array<{ role?: string; kind?: string; source_summary?: string }> = [];
  if (artifact.refs_meta_json) {
    try {
      const parsed = Array.isArray(artifact.refs_meta_json)
        ? artifact.refs_meta_json
        : JSON.parse(String(artifact.refs_meta_json));
      refs = parsed;
    } catch {
      refs = [];
    }
  }

  return (
    <div>
      <div style={{ marginBottom: 16 }}>
        <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>PROMPT</div>
        <div
          data-testid="trace-prompt-text"
          style={{
            background: '#0d1117',
            border: '1px solid #30363d',
            borderRadius: 6,
            padding: 12,
            color: '#e1e4e8',
            fontSize: 13,
            lineHeight: 1.6,
            whiteSpace: 'pre-wrap',
            minHeight: 40,
          }}
        >
          {artifact.prompt || <span style={{ color: '#8b949e' }}>No prompt recorded</span>}
        </div>
      </div>

      {artifact.negative_prompt && (
        <div style={{ marginBottom: 16 }}>
          <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>NEGATIVE PROMPT</div>
          <div
            data-testid="trace-prompt-negative"
            style={{
              background: '#0d1117',
              border: '1px solid #30363d',
              borderRadius: 6,
              padding: 12,
              color: '#e1e4e8',
              fontSize: 13,
              lineHeight: 1.6,
              whiteSpace: 'pre-wrap',
            }}
          >
            {artifact.negative_prompt}
          </div>
        </div>
      )}

      {refs.length > 0 && (
        <div>
          <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>
            REFERENCE IMAGES ({refs.length})
          </div>
          {refs.map((ref, i) => (
            <div
              key={i}
              data-testid={`trace-prompt-ref-${i}`}
              style={{
                background: '#0d1117',
                border: '1px solid #30363d',
                borderRadius: 6,
                padding: 10,
                marginBottom: 8,
                fontSize: 12,
              }}
            >
              <span style={{ color: '#58a6ff' }}>role: </span>
              <span style={{ color: '#e1e4e8' }}>{ref.role || 'none'}</span>
              <span style={{ color: '#8b949e' }}> · </span>
              <span style={{ color: '#58a6ff' }}>kind: </span>
              <span style={{ color: '#e1e4e8' }}>{ref.kind}</span>
              <br />
              <span style={{ color: '#8b949e', fontSize: 11 }}>{ref.source_summary}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ParamsTab({ artifact, log }: { artifact: RequestArtifact | null; log: RequestLog | null }) {
  const params: Record<string, unknown> =
    artifact?.params_json && typeof artifact.params_json === 'object' && !Array.isArray(artifact.params_json)
      ? (artifact.params_json as Record<string, unknown>)
      : {};

  return (
    <div>
      <div style={{ marginBottom: 16 }}>
        <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>GENERATION PARAMS</div>
        <table
          data-testid="trace-params-table"
          style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}
        >
          <tbody>
            {Object.entries(params).map(([k, v]) => (
              v !== null && v !== undefined && (
                <tr key={k} style={{ borderBottom: '1px solid #21262d' }}>
                  <td style={{ padding: '6px 8px', color: '#8b949e', width: '40%' }}>{k}</td>
                  <td style={{ padding: '6px 8px', color: '#e1e4e8', wordBreak: 'break-all' }}>
                    {typeof v === 'object' ? JSON.stringify(v) : String(v)}
                  </td>
                </tr>
              )
            ))}
            {Object.keys(params).length === 0 && (
              <tr>
                <td colSpan={2} style={{ padding: 8, color: '#8b949e', fontSize: 12 }}>No params recorded</td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {log && (
        <div>
          <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>REQUEST METADATA</div>
          <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
            <tbody>
              {[
                ['Model', log.model],
                ['Provider', log.provider],
                ['Status', log.status],
                ['Media type', log.media_type],
                ['Cost', `$${log.cost_usd.toFixed(4)}`],
                ['Latency', `${log.latency_ms}ms`],
                ['Time', new Date(log.created_at).toLocaleString()],
              ].map(([k, v]) => (
                <tr key={k} style={{ borderBottom: '1px solid #21262d' }}>
                  <td style={{ padding: '6px 8px', color: '#8b949e', width: '40%' }}>{k}</td>
                  <td style={{ padding: '6px 8px', color: '#e1e4e8' }}>{v}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function ResponseTab({ log }: { log: RequestLog | null }) {
  if (!log) {
    return <div style={{ color: '#8b949e', fontSize: 13 }}>No log data available.</div>;
  }
  return (
    <div>
      <div style={{ color: '#8b949e', fontSize: 12, marginBottom: 6, fontWeight: 600 }}>FULL LOG JSON</div>
      <pre
        data-testid="trace-response-json"
        style={{
          background: '#0d1117',
          border: '1px solid #30363d',
          borderRadius: 6,
          padding: 12,
          margin: 0,
          fontSize: 12,
          color: '#e1e4e8',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-all',
          overflowX: 'auto',
        }}
      >
        {JSON.stringify(log, null, 2)}
      </pre>
    </div>
  );
}
