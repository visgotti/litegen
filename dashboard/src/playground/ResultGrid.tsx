import type { ResultTileState } from './types';
import ResultTile from './ResultTile';
import ResultTile3D from './ResultTile3D';

interface Props { tiles: ResultTileState[]; onRerun: (modelId: string) => void; }

export default function ResultGrid({ tiles, onRerun }: Props) {
  if (tiles.length === 0) {
    return <div className="pg-grid-empty" data-testid="pg-grid-empty">No results yet</div>;
  }
  return (
    <div className="pg-grid" data-testid="pg-result-grid">
      {tiles.map(t => t.mediaType === 'model3d'
        ? <ResultTile3D key={t.key} tile={t} onRerun={onRerun} />
        : <ResultTile key={t.key} tile={t} onRerun={onRerun} />)}
    </div>
  );
}
