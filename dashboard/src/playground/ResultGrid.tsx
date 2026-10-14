import type { ResultTileState } from './types';
import ResultTile from './ResultTile';
import ResultTile3D from './ResultTile3D';

interface Props {
  tiles: ResultTileState[];
  onRerun: (modelId: string) => void;
  /** While a run is in flight: a rerun replaces the whole run, abandoning
   *  every other tile's job. */
  rerunDisabled?: boolean;
}

export default function ResultGrid({ tiles, onRerun, rerunDisabled }: Props) {
  if (tiles.length === 0) {
    return <div className="pg-grid-empty" data-testid="pg-grid-empty">No results yet</div>;
  }
  return (
    <div className="pg-grid" data-testid="pg-result-grid">
      {tiles.map(t => t.mediaType === 'model3d'
        ? <ResultTile3D key={t.key} tile={t} onRerun={onRerun} rerunDisabled={rerunDisabled} />
        : <ResultTile key={t.key} tile={t} onRerun={onRerun} rerunDisabled={rerunDisabled} />)}
    </div>
  );
}
