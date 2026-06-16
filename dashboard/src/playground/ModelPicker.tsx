import { useMemo, useState } from 'react';
import type { ModelInfo } from '@litegen/sdk';
import { availabilityOf, sortModels } from './params';

/** Availability glyph + label shown as a small dot beside each model id. */
const DOT: Record<string, string> = { live: '●', mock: '◇', setup: '○' };
const DOT_LABEL: Record<string, string> = { live: 'live', mock: 'mock', setup: 'setup' };

/** Curated, distinct-on-dark palette. Providers map here by a stable hash so
 *  colors stay consistent and new providers get a color with zero maintenance. */
const PALETTE = [
  '#58a6ff', '#bc8cff', '#f778ba', '#ff7b72', '#ffa657', '#e3b341',
  '#7ee787', '#39c5cf', '#a5d6ff', '#d2a8ff', '#ffbedd', '#79c0ff',
  '#56d4dd', '#f0883e', '#db61a2', '#6cb6ff',
];

function providerColor(provider: string): string {
  let h = 0;
  for (let i = 0; i < provider.length; i++) h = (h * 31 + provider.charCodeAt(i)) >>> 0;
  return PALETTE[h % PALETTE.length];
}

interface Group {
  provider: string;
  models: ModelInfo[];
}

interface Props {
  models: ModelInfo[];
  selected: string[];
  onToggle: (id: string) => void;
  onToggleGroup: (ids: string[]) => void;
}

export default function ModelPicker({ models, selected, onToggle, onToggleGroup }: Props) {
  const [filter, setFilter] = useState('');
  const sorted = useMemo(() => sortModels(models), [models]);

  // Filter, then group by provider preserving the sorted (live → mock → setup → alpha) order.
  const groups = useMemo<Group[]>(() => {
    const q = filter.toLowerCase();
    const byProvider = new Map<string, ModelInfo[]>();
    for (const m of sorted) {
      if (!m.id.toLowerCase().includes(q)) continue;
      const list = byProvider.get(m.provider);
      if (list) list.push(m);
      else byProvider.set(m.provider, [m]);
    }
    return [...byProvider.entries()].map(([provider, ms]) => ({ provider, models: ms }));
  }, [sorted, filter]);

  const selectedSet = new Set(selected);

  return (
    <div className="pg-picker" data-testid="pg-model-picker">
      <input className="input" data-testid="pg-model-filter" placeholder="Filter models…"
        value={filter} onChange={e => setFilter(e.target.value)} />
      <div className="pg-picker-count" data-testid="pg-selected-count">{selected.length} selected</div>
      <div className="pg-picker-list">
        {groups.map(g => {
          const color = providerColor(g.provider);
          const ids = g.models.map(m => m.id);
          const allSelected = ids.every(id => selectedSet.has(id));
          return (
            <div key={g.provider} className="pg-provider-group">
              <button type="button" className="pg-provider-header"
                data-testid={`pg-provider-${g.provider}`}
                onClick={() => onToggleGroup(ids)}
                title={allSelected ? `Deselect all ${g.provider}` : `Select all ${g.provider}`}>
                <span className="pg-provider-badge"
                  style={{ color, borderColor: color, background: `${color}1f` }}>
                  {g.provider}
                </span>
                {allSelected && <span className="pg-all-badge" data-testid={`pg-all-${g.provider}`}>ALL</span>}
              </button>
              {g.models.map(m => {
                const av = availabilityOf(m);
                return (
                  <label key={m.id} className="pg-picker-row" data-testid={`pg-model-${m.id}`}>
                    <input type="checkbox" checked={selectedSet.has(m.id)} onChange={() => onToggle(m.id)} />
                    <span className={`pg-avail-dot pg-avail-${av}`} title={DOT_LABEL[av]}>{DOT[av]}</span>
                    <span className="pg-picker-id">{m.id}</span>
                  </label>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
