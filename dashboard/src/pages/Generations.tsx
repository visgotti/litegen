import React, { useEffect, useState, useCallback } from 'react';
import { client } from '../sdk-client';
import type { Generation, PaginatedResponse } from '@litegen/sdk';
import { useAutoRefresh } from '../hooks/useAutoRefresh';
import ModelViewer, { meshUrl, previewUrl } from '../components/ModelViewer';

const ACTIVE_STATUSES = new Set(['pending', 'processing']);

function StatusBadge({ id, status }: { id: string; status: string }) {
  return (
    <span
      data-testid={`gen-status-${id}`}
      className={`status-badge ${status}`}
    >
      {status}
    </span>
  );
}

/** Inline media for a generation: <img> for images, <video> for video,
 *  <model-viewer> for meshes. `thumb` renders the small in-row preview —
 *  always a flat 2D image, never a GL canvas per row. */
export function MediaPreview({ g, thumb }: { g: Generation; thumb?: boolean }) {
  if (g.status !== 'completed' || !g.result_url) {
    return thumb ? <span style={{ color: '#8b949e' }}>—</span> : null;
  }
  const style = { maxWidth: '100%', maxHeight: 480, marginBottom: 12, display: 'block', borderRadius: 6 } as const;

  if (g.media_type === 'model3d') {
    const mesh = meshUrl(g);
    const poster = previewUrl(g);
    if (thumb) {
      return poster
        ? <img src={poster} alt="" loading="lazy" data-testid={`gen-thumb-${g.id}`}
               style={{ width: 48, height: 48, objectFit: 'cover', borderRadius: 4, display: 'block' }} />
        : <span title="3D result" aria-label="3d">🧊</span>;
    }
    return mesh
      ? <ModelViewer src={mesh} poster={poster} testId={`gen-media-${g.id}`} style={style} />
      : null;
  }

  const isVideo = g.media_type === 'video';
  if (thumb) {
    if (isVideo) return <span title="video result" aria-label="video">🎬</span>;
    return (
      <img src={g.result_url} alt="" loading="lazy" data-testid={`gen-thumb-${g.id}`}
           style={{ width: 48, height: 48, objectFit: 'cover', borderRadius: 4, display: 'block' }} />
    );
  }
  return isVideo
    ? <video controls src={g.result_url} data-testid={`gen-media-${g.id}`} style={style} />
    : <img src={g.result_url} alt={g.model} loading="lazy" data-testid={`gen-media-${g.id}`} style={style} />;
}

export default function Generations() {
  const [data, setData] = useState<PaginatedResponse<Generation> | null>(null);
  const [page, setPage] = useState(1);
  const [error, setError] = useState('');
  const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());
  const [cancelling, setCancelling] = useState<Set<string>>(new Set());

  const load = useCallback((p: number) => {
    client.generations.list({ page: p, per_page: 25 })
      .then(d => { setData(d); setPage(p); })
      .catch(e => setError(e.message));
  }, []);

  useEffect(() => { load(1); }, [load]);

  const hasActive = data?.data.some(g => ACTIVE_STATUSES.has(g.status)) ?? false;

  useAutoRefresh(() => load(page), 3000, hasActive);

  const toggleExpand = (id: string) => {
    setExpandedIds(prev => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const handleCancel = async (id: string) => {
    setCancelling(prev => new Set(prev).add(id));
    try {
      await client.generations.cancel(id);
      load(page);
    } catch (e: unknown) {
      setError((e as Error).message);
    } finally {
      setCancelling(prev => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  };

  if (error) return <div className="alert alert-error">{error}</div>;
  if (!data) return <div className="loading">Loading generations...</div>;

  return (
    <div>
      <h2 className="page-title">Generations</h2>

      <div className="table-container">
        <table data-testid="gen-table">
          <thead>
            <tr>
              <th>ID</th>
              <th>Model</th>
              <th>Preview</th>
              <th>Status</th>
              <th>Cost</th>
              <th>Created</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {data.data.length === 0 && (
              <tr>
                <td colSpan={7} style={{ textAlign: 'center', color: '#8b949e' }}>
                  No generations yet
                </td>
              </tr>
            )}
            {data.data.map(g => (
              <React.Fragment key={g.id}>
                <tr
                  data-testid={`gen-row-${g.id}`}
                  style={{ cursor: 'pointer' }}
                >
                  <td
                    data-testid={`gen-expand-${g.id}`}
                    onClick={() => toggleExpand(g.id)}
                    title="Click to expand"
                    style={{ fontFamily: 'monospace', fontSize: 12, maxWidth: 160, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                  >
                    {expandedIds.has(g.id) ? '▼ ' : '▶ '}{g.id}
                  </td>
                  <td><code style={{ fontSize: 12 }}>{g.model}</code></td>
                  <td
                    onClick={() => toggleExpand(g.id)}
                    title="Click to expand"
                    style={{ cursor: 'pointer' }}
                  >
                    <MediaPreview g={g} thumb />
                  </td>
                  <td><StatusBadge id={g.id} status={g.status} /></td>
                  <td>${(g.cost_usd ?? 0).toFixed(4)}</td>
                  <td style={{ fontSize: 12, whiteSpace: 'nowrap' }}>
                    {new Date(g.created_at).toLocaleString()}
                  </td>
                  <td>
                    {ACTIVE_STATUSES.has(g.status) && (
                      <button
                        data-testid={`gen-cancel-${g.id}`}
                        className="btn btn-danger"
                        style={{ fontSize: 12, padding: '4px 10px' }}
                        disabled={cancelling.has(g.id)}
                        onClick={() => handleCancel(g.id)}
                      >
                        {cancelling.has(g.id) ? 'Cancelling…' : 'Cancel'}
                      </button>
                    )}
                  </td>
                </tr>
                {expandedIds.has(g.id) && (
                  <tr className="gen-detail-row" key={`${g.id}-detail`}>
                    <td colSpan={7} data-testid={`gen-detail-${g.id}`}>
                      <MediaPreview g={g} />
                      <pre>{JSON.stringify(g, null, 2)}</pre>
                    </td>
                  </tr>
                )}
              </React.Fragment>
            ))}
          </tbody>
        </table>
      </div>

      {data.total_pages > 1 && (
        <div className="pagination">
          <button
            data-testid="gen-pagination-prev"
            className="btn btn-secondary"
            disabled={page <= 1}
            onClick={() => load(page - 1)}
          >
            Previous
          </button>
          <span
            data-testid="gen-pagination-info"
            style={{ padding: '8px 12px', color: '#8b949e' }}
          >
            Page {page} of {data.total_pages}
          </span>
          <button
            data-testid="gen-pagination-next"
            className="btn btn-secondary"
            disabled={page >= data.total_pages}
            onClick={() => load(page + 1)}
          >
            Next
          </button>
        </div>
      )}
    </div>
  );
}
