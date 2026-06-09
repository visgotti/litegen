import type { ResultTileState } from './types';
import ResultTile from './ResultTile';

interface Props { tiles: ResultTileState[]; onRerun: (modelId: string) => void; }

export default function ResultGrid({ tiles, onRerun }: Props) {
  if (tiles.length === 0) {
    return <div className="pg-grid-empty" data-testid="pg-grid-empty">No results yet</div>;
  }
  return (
    <div className="pg-grid" data-testid="pg-result-grid">
      {tiles.map(t => <ResultTile key={t.key} tile={t} onRerun={onRerun} />)}
    </div>
  );
}
