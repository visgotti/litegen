import { useEffect, useState } from 'react';
import { Trash2 } from 'lucide-react';
import { client } from '../sdk-client';
import type { ModelInfo, Model3dGenerationRequest } from '@litegen/sdk';
import ModelPicker from './ModelPicker';
import ParamField from './ParamField';
import RefImagesField from './RefImagesField';
import ExtraParamsField from './ExtraParamsField';
import ResultGrid from './ResultGrid';
import { useUnifiedParams } from './useUnifiedParams';
import { useFanOut } from './useFanOut';
import { promptUsage } from './params';
import {
  getMultiHistory,
  pushMultiHistory,
  removeMultiHistory,
} from '../playground-history';
import type { MultiRunHistoryEntry } from '../playground-history';
import { withoutInlineRefs } from './ref-images';

type ReqView = 'results' | 'requests';

export default function CompareMode() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [view, setView] = useState<ReqView>('results');
  const [error, setError] = useState('');
  const [estCost, setEstCost] = useState<number | null>(null);
  const [history, setHistory] = useState<MultiRunHistoryEntry[]>([]);
  const {
    merged, roles, maxRefs, extras, form, params, setForm, setParam, resetParam, problems, buildRequests, schemasById,
  } = useUnifiedParams(selected);
  // Models that take no prompt never receive the shared one (see
  // promptForModel); the box only disappears when no selected model takes it.
  const noPrompt = selected.filter(id => schemasById[id] && promptUsage(schemasById[id]) === 'unused');
  const promptHidden = selected.length > 0 && noPrompt.length === selected.length;
  const { tiles, running, run, cancel } = useFanOut();

  useEffect(() => {
    client.models.list()
      .then(all => setModels(all.filter(m => m.media_type === 'image' || m.media_type === 'model3d')))
      .catch(e => setError(e.message));
    setHistory(getMultiHistory());
  }, []);

  const toggle = (id: string) =>
    setSelected(prev => prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]);

  // Header click: select every model in the group, or deselect all if already full.
  const toggleGroup = (ids: string[]) =>
    setSelected(prev => {
      if (ids.every(id => prev.includes(id))) return prev.filter(id => !ids.includes(id));
      const set = new Set(prev);
      ids.forEach(id => set.add(id));
      return [...set];
    });

  // buildRequests (useUnifiedParams) doesn't know media family — look it up
  // from the loaded model list. Shared by the request builder, the cost
  // preview and generate() so a 3D selection gets 3D-shaped bodies and goes to
  // the 3D client calls instead of the image ones.
  const mediaTypeOf = (modelId: string): 'image' | 'model3d' =>
    models.find(m => m.id === modelId)?.media_type === 'model3d' ? 'model3d' : 'image';

  const requests = buildRequests(selected, mediaTypeOf);
  const sig = JSON.stringify(requests);
  // A model the server would reject (no prompt where one is required, a
  // missing input image) blocks the run instead of failing its tile.
  const blocked = problems(selected).filter(p => p.problems.length > 0);
  const anyImageModel = selected.some(id => mediaTypeOf(id) === 'image');
  const any3dModel = selected.some(id => mediaTypeOf(id) === 'model3d');

  // Debounced cost preview across all selected models.
  useEffect(() => {
    if (requests.length === 0 || blocked.length > 0) { setEstCost(null); return; }
    const handle = setTimeout(async () => {
      try {
        const estimates = await Promise.all(requests.map(r => {
          const estimate = mediaTypeOf(r.modelId) === 'model3d'
            ? client.models3d.estimateCost(r.request as Model3dGenerationRequest)
            : client.images.estimateCost(r.request);
          return estimate
            .then(c => (c as { total_cost_usd?: number }).total_cost_usd ?? 0)
            .catch(() => 0);
        }));
        setEstCost(estimates.reduce((a, b) => a + b, 0));
      } catch {
        setEstCost(null);
      }
    }, 400);
    return () => clearTimeout(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, blocked.length]);

  const generate = async (only?: string) => {
    setError('');
    const picked = only ? requests.filter(r => r.modelId === only) : requests;
    if (picked.length === 0 || picked.some(r => blocked.some(b => b.modelId === r.modelId))) return;
    setView('results');
    const reqs = picked.map(r => ({ ...r, mediaType: mediaTypeOf(r.modelId) }));
    await run(reqs);
    const entry: MultiRunHistoryEntry = {
      id: crypto.randomUUID(),
      kind: 'multi',
      prompt: form.prompt,
      timestamp: new Date().toISOString(),
      models: reqs.map(r => r.modelId),
      results: reqs.map(r => ({
        model: r.modelId,
        request: withoutInlineRefs(r.request as unknown as Record<string, unknown>),
      })),
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
          <ModelPicker models={models} selected={selected} onToggle={toggle} onToggleGroup={toggleGroup} />

          {promptHidden ? (
            <p className="pg-param-description" data-testid="pg-prompt-unused" style={{ margin: '0 0 12px' }}>
              {selected.length === 1 ? 'This model takes' : 'These models take'} no prompt — they generate from the input image.
            </p>
          ) : (
            <div className="playground-form-group">
              <label htmlFor="pg-prompt-input">Prompt</label>
              <textarea id="pg-prompt-input" data-testid="pg-prompt" rows={3} value={form.prompt}
                onChange={e => setForm(prev => ({ ...prev, prompt: e.target.value }))}
                placeholder={any3dModel && !anyImageModel ? 'Describe the 3D model…' : 'Describe the image…'} />
              {noPrompt.length > 0 && (
                <span className="pg-param-description" data-testid="pg-prompt-not-sent">
                  Not sent to {noPrompt.map(m => m.split('/').pop()).join(', ')} — {noPrompt.length === 1 ? 'it takes' : 'they take'} no prompt.
                </span>
              )}
            </div>
          )}

          <RefImagesField roles={roles} refs={form.refs} maxTotal={maxRefs} totalSelected={selected.length}
            disabled={running}
            onChange={refs => setForm(prev => ({ ...prev, refs }))} />

          {merged.length > 0 && (
            <div className="pg-params" data-testid="pg-params">
              {merged.map(p => (
                <ParamField key={p.name} name={p.name} spec={p.spec} models={p.models}
                  totalSelected={selected.length} value={params[p.name]}
                  onChange={v => setParam(p.name, v)}
                  onReset={p.name in form.params ? () => resetParam(p.name) : undefined} />
              ))}
            </div>
          )}

          <ExtraParamsField extras={extras} values={form.extra} totalSelected={selected.length}
            disabled={running}
            onChange={(key, raw) => setForm(prev => ({ ...prev, extra: { ...prev.extra, [key]: raw } }))} />

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
                value={form.n} disabled={!anyImageModel && any3dModel}
                onChange={e => setForm(prev => ({ ...prev, n: parseInt(e.target.value, 10) || 1 }))} />
            </div>
          </div>
          {any3dModel && (
            <p className="pg-param-description" data-testid="pg-n-note" style={{ margin: '-4px 0 12px' }}>
              N applies to image models only — one 3D job returns one result.
            </p>
          )}

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

          {blocked.length > 0 && (
            <ul className="pg-problems" data-testid="pg-problems">
              {blocked.map(b => (
                <li key={b.modelId} data-testid={`pg-problems-${b.modelId}`}>
                  <code>{b.modelId}</code> {b.problems.join(', ')}
                </li>
              ))}
            </ul>
          )}

          {running ? (
            <button className="btn btn-danger" style={{ width: '100%' }}
              data-testid="pg-cancel" onClick={cancel}>Cancel</button>
          ) : (
            <button className="btn btn-primary" style={{ width: '100%' }} data-testid="pg-generate"
              disabled={selected.length === 0 || blocked.length > 0} onClick={() => generate()}>
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
            <ResultGrid tiles={tiles} onRerun={id => generate(id)} rerunDisabled={running} />
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
