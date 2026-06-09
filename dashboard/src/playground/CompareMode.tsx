import { useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { client } from '../sdk-client';
import type { ModelInfo } from '@litegen/sdk';
import ModelPicker from './ModelPicker';
import ParamField from './ParamField';
import ResultGrid from './ResultGrid';
import { useUnifiedParams } from './useUnifiedParams';
import { useFanOut } from './useFanOut';
import {
  getMultiHistory,
  pushMultiHistory,
  removeMultiHistory,
} from '../playground-history';
import type { MultiRunHistoryEntry } from '../playground-history';

type ReqView = 'results' | 'requests';

export default function CompareMode() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [view, setView] = useState<ReqView>('results');
  const [error, setError] = useState('');
  const [estCost, setEstCost] = useState<number | null>(null);
  const [history, setHistory] = useState<MultiRunHistoryEntry[]>([]);
  const { merged, form, params, setForm, setParam, buildRequests } = useUnifiedParams(selected);
  const { tiles, running, run, cancel } = useFanOut();

  useEffect(() => {
    client.models.list()
      .then(all => setModels(all.filter(m => m.media_type === 'image')))
      .catch(e => setError(e.message));
    setHistory(getMultiHistory());
  }, []);

  const toggle = (id: string) =>
    setSelected(prev => prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]);

  const requests = buildRequests(selected);
  const sig = JSON.stringify(requests);

  // Debounced cost preview across all selected models.
  useEffect(() => {
    if (requests.length === 0 || !form.prompt.trim()) { setEstCost(null); return; }
    const handle = setTimeout(async () => {
      try {
        const estimates = await Promise.all(requests.map(r =>
          client.images.estimateCost(r.request)
            .then(c => (c as { total_cost_usd?: number }).total_cost_usd ?? 0)
            .catch(() => 0),
        ));
        setEstCost(estimates.reduce((a, b) => a + b, 0));
      } catch {
        setEstCost(null);
      }
    }, 400);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, form.prompt]);

  const generate = async (only?: string) => {
    setError('');
    const reqs = only ? requests.filter(r => r.modelId === only) : requests;
    if (reqs.length === 0 || !form.prompt.trim()) return;
    setView('results');
    await run(reqs);
    const entry: MultiRunHistoryEntry = {
      id: crypto.randomUUID(),
      kind: 'multi',
      prompt: form.prompt,
      timestamp: new Date().toISOString(),
      models: reqs.map(r => r.modelId),
      results: reqs.map(r => ({ model: r.modelId, request: r.request as Record<string, unknown> })),
    };
    pushMultiHistory(entry);
    setHistory(getMultiHistory());
  };

  const restoreRun = (entry: MultiRunHistoryEntry) => {
    setSelected(entry.models);
    setForm(prev => ({ ...prev, prompt: entry.prompt }));
  };

  const deleteRun = (id: string, e: React.MouseEvent) => {
    e.stopPropagation();
    removeMultiHistory(id);
    setHistory(getMultiHistory());
  };

  return (
    <div>
      <div className="playground-layout">
        {/* Left column — controls */}
        <div className="playground-panel">
          {error && <div className="alert alert-error">{error}</div>}
          <ModelPicker models={models} selected={selected} onToggle={toggle} />

          <div className="playground-form-group">
            <label>Prompt</label>
            <textarea data-testid="pg-prompt" rows={3} value={form.prompt}
              onChange={e => setForm(prev => ({ ...prev, prompt: e.target.value }))}
              placeholder="Describe the image…" />
          </div>

          {merged.length > 0 && (
            <div className="pg-params" data-testid="pg-params">
              {merged.map(p => (
                <ParamField key={p.name} name={p.name} spec={p.spec} models={p.models}
                  totalSelected={selected.length} value={params[p.name]}
                  onChange={v => setParam(p.name, v)} />
              ))}
            </div>
          )}

          <div className="playground-form-row">
            <div className="playground-form-group">
              <label>Seed</label>
              <input type="number" className="input" data-testid="pg-seed" placeholder="Random"
                style={{ width: '100%', boxSizing: 'border-box' }}
                value={form.seed} onChange={e => setForm(prev => ({ ...prev, seed: e.target.value }))} />
            </div>
            <div className="playground-form-group">
              <label>N (1–4)</label>
              <input type="number" className="input" data-testid="pg-n" min={1} max={4}
                style={{ width: '100%', boxSizing: 'border-box' }}
                value={form.n}
                onChange={e => setForm(prev => ({ ...prev, n: parseInt(e.target.value, 10) || 1 }))} />
            </div>
          </div>

          <div className="playground-form-group">
            <div className="toggle-row">
              <input type="checkbox" id="pg-strict-toggle" data-testid="pg-strict"
                checked={form.strict}
                onChange={e => setForm(prev => ({ ...prev, strict: e.target.checked }))} />
              <label htmlFor="pg-strict-toggle" style={{ textTransform: 'none', fontSize: 14, letterSpacing: 0 }}>
                Strict mode
              </label>
            </div>
          </div>

          {estCost != null && (
            <div className="pg-cost" data-testid="pg-cost">Est. cost: ${estCost.toFixed(3)}</div>
          )}

          {running ? (
            <button className="btn btn-danger" style={{ width: '100%' }}
              data-testid="pg-cancel" onClick={cancel}>Cancel</button>
          ) : (
            <button className="btn btn-primary" style={{ width: '100%' }} data-testid="pg-generate"
              disabled={selected.length === 0 || !form.prompt.trim()} onClick={() => generate()}>
              Generate {selected.length} model{selected.length === 1 ? '' : 's'}
            </button>
          )}
        </div>

        {/* Right column — results / requests */}
        <div className="playground-panel">
          <div className="playground-tabs">
            <button className={`playground-tab${view === 'results' ? ' active' : ''}`}
              data-testid="pg-view-results" onClick={() => setView('results')}>Results</button>
            <button className={`playground-tab${view === 'requests' ? ' active' : ''}`}
              data-testid="pg-view-requests" onClick={() => setView('requests')}>Requests</button>
          </div>
          {view === 'results' ? (
            <ResultGrid tiles={tiles} onRerun={id => generate(id)} />
          ) : (
            <pre data-testid="pg-requests-json" className="pg-requests">
              {JSON.stringify(requests, null, 2)}
            </pre>
          )}
        </div>
      </div>

      {/* Recent compare runs */}
      {history.length > 0 && (
        <div className="playground-history">
          <h3>Recent compare runs</h3>
          {history.slice().reverse().map((entry, i) => (
            <div key={entry.id} data-testid={`pg-history-row-${i}`} className="playground-history-row"
              onClick={() => restoreRun(entry)}>
              <div className="history-prompt">{entry.prompt}</div>
              <div className="history-meta">
                <code style={{ fontSize: 11 }}>{entry.models.length} models</code>
                {' · '}
                {new Date(entry.timestamp).toLocaleTimeString()}
              </div>
              <button className="btn btn-danger" style={{ fontSize: 12, padding: '4px 10px' }}
                data-testid={`pg-history-delete-${i}`} onClick={e => deleteRun(entry.id, e)} title="Delete">
                <Trash2 size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
