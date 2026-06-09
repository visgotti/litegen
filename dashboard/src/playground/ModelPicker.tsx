import { useMemo, useState } from 'react';
import type { ModelInfo } from '@litegen/sdk';
import { availabilityOf, sortModels } from './params';

const BADGE: Record<string, string> = { live: '● live', mock: '◇ mock', setup: '○ setup' };

interface Props {
  models: ModelInfo[];
  selected: string[];
  onToggle: (id: string) => void;
}

export default function ModelPicker({ models, selected, onToggle }: Props) {
  const [filter, setFilter] = useState('');
  const sorted = useMemo(() => sortModels(models), [models]);
  const shown = sorted.filter(m => m.id.toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="pg-picker" data-testid="pg-model-picker">
      <input className="input" data-testid="pg-model-filter" placeholder="Filter models…"
        value={filter} onChange={e => setFilter(e.target.value)} />
      <div className="pg-picker-count" data-testid="pg-selected-count">{selected.length} selected</div>
      <div className="pg-picker-list">
        {shown.map(m => {
          const av = availabilityOf(m);
          return (
            <label key={m.id} className="pg-picker-row" data-testid={`pg-model-${m.id}`}>
              <input type="checkbox" checked={selected.includes(m.id)} onChange={() => onToggle(m.id)} />
              <span className="pg-picker-id">{m.id}</span>
              <span className={`pg-badge pg-badge-${av}`}>{BADGE[av]}</span>
            </label>
          );
        })}
      </div>
    </div>
  );
}
