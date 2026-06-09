import type { ResultTileState } from './types';

interface Props { tile: ResultTileState; onRerun: (modelId: string) => void; }

export default function ResultTile({ tile, onRerun }: Props) {
  const src = tile.b64_json ? `data:image/png;base64,${tile.b64_json}` : tile.url ?? undefined;
  return (
    <div className="pg-tile" data-testid={`pg-tile-${tile.modelId}`}>
      <div className="pg-tile-head">
        <code className="pg-tile-model">{tile.modelId}</code>
        <button className="btn btn-secondary pg-tile-rerun" title="Rerun this model"
          data-testid={`pg-tile-rerun-${tile.modelId}`} onClick={() => onRerun(tile.modelId)}>↻</button>
      </div>
      <div className="pg-tile-image">
        {tile.status === 'running' || tile.status === 'queued' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-spinner-${tile.modelId}`}>⟳ generating…</span>
        ) : tile.status === 'error' ? (
          <span className="pg-tile-error" data-testid={`pg-tile-error-${tile.modelId}`}>⚠ {tile.error}</span>
        ) : src ? (
          <img data-testid={`pg-tile-img-${tile.modelId}`} src={src} alt={tile.modelId} />
        ) : (
          <span className="pg-tile-status">no image</span>
        )}
      </div>
      <div className="pg-tile-meta">
        {tile.costUsd != null && <span>${tile.costUsd.toFixed(3)}</span>}
        {tile.latencyMs != null && <span>{tile.latencyMs}ms</span>}
      </div>
    </div>
  );
}
