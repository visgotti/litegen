import ModelViewer from '../components/ModelViewer';
import { meshOf, previewOf, validAssets } from '../components/model3d-assets';
import type { ResultTileState } from './types';

interface Props { tile: ResultTileState; onRerun: (modelId: string) => void; }

/** Compare-mode grid tile. Deliberately the compact `ModelViewer`, not the
 *  `ModelPreview` inspector: several live canvases each with a full toolbar
 *  would crowd the grid, and the single-mode panel is where inspection lives. */
export default function ResultTile3D({ tile, onRerun }: Props) {
  // tile.assets comes straight from the poll response (useFanOut) and is not
  // validated there; a malformed entry must not throw in render.
  const assets = validAssets(tile.assets);
  const meshAsset = meshOf(assets, tile.url);
  const mesh = meshAsset?.url;
  const poster = previewOf(assets)?.url;
  return (
    <div className="pg-tile" data-testid={`pg-tile-${tile.modelId}`}>
      <div className="pg-tile-head">
        <code className="pg-tile-model">{tile.modelId}</code>
        <button className="btn btn-secondary pg-tile-rerun" title="Rerun this model"
          data-testid={`pg-tile-rerun-${tile.modelId}`} onClick={() => onRerun(tile.modelId)}>↻</button>
      </div>
      <div className="pg-tile-image">
        {tile.status === 'queued' || tile.status === 'running' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-spinner-${tile.modelId}`}>⟳ submitting…</span>
        ) : tile.status === 'polling' ? (
          <span className="pg-tile-status" data-testid={`pg-tile-progress-${tile.modelId}`}>
            ⟳ generating… {tile.progress ?? 0}%
          </span>
        ) : tile.status === 'error' ? (
          <span className="pg-tile-error" data-testid={`pg-tile-error-${tile.modelId}`}>⚠ {tile.error}</span>
        ) : mesh ? (
          <ModelViewer src={mesh} poster={poster} format={meshAsset?.format} testId={`pg-tile-mesh-${tile.modelId}`} style={{ height: 240 }} />
        ) : (
          <span className="pg-tile-status">no mesh</span>
        )}
      </div>
      <div className="pg-tile-meta">
        {tile.costUsd != null && <span>${tile.costUsd.toFixed(3)}</span>}
        {tile.latencyMs != null && <span>{tile.latencyMs}ms</span>}
      </div>
    </div>
  );
}
