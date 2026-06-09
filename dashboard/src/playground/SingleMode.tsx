import { useEffect, useState, useCallback } from 'react';
import { Trash2, RotateCcw } from 'lucide-react';
import { client } from '../sdk-client';
import {
  getPlaygroundHistory,
  pushPlaygroundHistory,
  removePlaygroundHistory,
} from '../playground-history';
import type { PlaygroundHistoryEntry } from '../playground-history';
import type { ModelInfo } from '@litegen/sdk';

type Tab = 'image' | 'request' | 'response';

interface FormState {
  model: string;
  prompt: string;
  negativePrompt: string;
  seed: string;
  size: string;
  n: number;
  strict: boolean;
}

const DEFAULT_FORM: FormState = {
  model: '',
  prompt: '',
  negativePrompt: '',
  seed: '',
  size: '',
  n: 1,
  strict: true,
};

export default function SingleMode() {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [sizeOptions, setSizeOptions] = useState<string[]>([]);
  const [form, setForm] = useState<FormState>(DEFAULT_FORM);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<Tab>('image');
  const [requestJson, setRequestJson] = useState<string>('');
  const [responseJson, setResponseJson] = useState<string>('');
  const [imageData, setImageData] = useState<{ b64_json?: string | null; url?: string | null } | null>(null);
  const [history, setHistory] = useState<PlaygroundHistoryEntry[]>([]);
  const [error, setError] = useState('');

  // Load image models on mount
  useEffect(() => {
    client.models.list()
      .then(allModels => {
        const imageModels = allModels.filter(
          m => m.media_type === 'image' && m.provider === 'mock',
        );
        setModels(imageModels);
        if (imageModels.length > 0) {
          // Prefer the visual mock model so the Playground shows real visible output
          const visual = imageModels.find(m => m.id === 'mock/visual-image-gen');
          const defaultModel = visual ?? imageModels[0];
          setForm(prev => ({ ...prev, model: defaultModel.id }));
        }
      })
      .catch(e => setError(e.message));
  }, []);

  // Load size options when model changes
  useEffect(() => {
    if (!form.model) return;
    client.models.getSchema(form.model)
      .then((schema: unknown) => {
        const s = schema as Record<string, unknown>;
        const params = s?.params as Record<string, unknown> | undefined;
        const sizeParam = params?.size as Record<string, unknown> | undefined;
        const rawValues = sizeParam?.values as Array<unknown> | undefined;
        // Schema returns sizes as [w, h] tuples; convert to "WxH" strings for the API.
        const values: string[] | undefined = rawValues?.map(v =>
          Array.isArray(v) ? `${v[0]}x${v[1]}` : String(v),
        );
        if (values && values.length > 0) {
          setSizeOptions(values);
          setForm(prev => ({ ...prev, size: values[0] }));
        } else {
          setSizeOptions([]);
          setForm(prev => ({ ...prev, size: '' }));
        }
      })
      .catch(() => {
        setSizeOptions([]);
        setForm(prev => ({ ...prev, size: '' }));
      });
  }, [form.model]);

  // Load history on mount
  useEffect(() => {
    setHistory(getPlaygroundHistory());
  }, []);

  const refreshHistory = useCallback(() => {
    setHistory(getPlaygroundHistory());
  }, []);

  const handleGenerate = async () => {
    if (!form.prompt.trim()) return;
    setLoading(true);
    setError('');

    const body: Record<string, unknown> = {
      model: form.model,
      prompt: form.prompt,
      n: form.n,
      strict: form.strict,
    };
    if (form.negativePrompt) body.negative_prompt = form.negativePrompt;
    if (form.seed) body.seed = parseInt(form.seed, 10);
    if (form.size) body.size = form.size;

    setRequestJson(JSON.stringify(body, null, 2));

    try {
      // Cast body to ImageGenerationRequest — the server accepts extra fields gracefully.
      const res = await client.images.generate(body as Parameters<typeof client.images.generate>[0]);
      setResponseJson(JSON.stringify(res, null, 2));
      setImageData(res.data?.[0] ?? null);
      setActiveTab('image');

      // Save to history
      const entry: PlaygroundHistoryEntry = {
        id: crypto.randomUUID(),
        model: form.model,
        prompt: form.prompt,
        timestamp: new Date().toISOString(),
        requestBody: body,
        responseBody: res as Record<string, unknown>,
      };
      pushPlaygroundHistory(entry);
      refreshHistory();
    } catch (e: unknown) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const handleHistoryClick = (entry: PlaygroundHistoryEntry) => {
    const req = entry.requestBody;
    setForm(prev => ({
      ...prev,
      model: entry.model,
      prompt: entry.prompt,
      negativePrompt: (req.negative_prompt as string) ?? '',
      seed: req.seed != null ? String(req.seed) : '',
      size: (req.size as string) ?? prev.size,
      n: (req.n as number) ?? 1,
      strict: req.strict != null ? (req.strict as boolean) : true,
    }));
    setRequestJson(JSON.stringify(req, null, 2));
    setResponseJson(JSON.stringify(entry.responseBody, null, 2));
    const respData = entry.responseBody as { data?: Array<{ b64_json?: string; url?: string }> };
    setImageData(respData?.data?.[0] ?? null);
    setActiveTab('image');
  };

  const handleResubmit = async (entry: PlaygroundHistoryEntry) => {
    handleHistoryClick(entry);
    // Small delay to let state settle before firing
    await new Promise(r => setTimeout(r, 0));
    const body = entry.requestBody;
    setLoading(true);
    setError('');
    setRequestJson(JSON.stringify(body, null, 2));
    try {
      const res = await client.images.generate(body as Parameters<typeof client.images.generate>[0]);
      setResponseJson(JSON.stringify(res, null, 2));
      setImageData(res.data?.[0] ?? null);
      setActiveTab('image');

      const newEntry: PlaygroundHistoryEntry = {
        id: crypto.randomUUID(),
        model: entry.model,
        prompt: entry.prompt,
        timestamp: new Date().toISOString(),
        requestBody: body,
        responseBody: res as Record<string, unknown>,
      };
      pushPlaygroundHistory(newEntry);
      refreshHistory();
    } catch (e: unknown) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  const handleDeleteHistory = (id: string, e: React.MouseEvent) => {
    e.stopPropagation();
    removePlaygroundHistory(id);
    refreshHistory();
  };

  return (
    <div>
      {error && <div className="alert alert-error">{error}</div>}

      <div className="playground-layout">
        {/* Left column — Input form */}
        <div className="playground-panel">
          <div className="playground-form-group">
            <label>Model</label>
            <select
              data-testid="playground-model"
              className="input"
              style={{ width: '100%' }}
              value={form.model}
              onChange={e => setForm(prev => ({ ...prev, model: e.target.value }))}
            >
              {models.map(m => (
                <option key={m.id} value={m.id}>{m.id}</option>
              ))}
            </select>
            {(() => {
              const selected = models.find(m => m.id === form.model);
              return selected?.description ? (
                <p
                  data-testid="playground-model-description"
                  style={{ margin: '4px 0 0', fontSize: 12, color: 'var(--text-muted, #888)', lineHeight: 1.4 }}
                >
                  {selected.description}
                </p>
              ) : null;
            })()}
          </div>

          <div className="playground-form-group">
            <label>Prompt</label>
            <textarea
              data-testid="playground-prompt"
              rows={3}
              required
              value={form.prompt}
              onChange={e => setForm(prev => ({ ...prev, prompt: e.target.value }))}
              placeholder="Describe the image..."
            />
          </div>

          <div className="playground-form-group">
            <label>Negative Prompt (optional)</label>
            <textarea
              data-testid="playground-negative-prompt"
              rows={2}
              value={form.negativePrompt}
              onChange={e => setForm(prev => ({ ...prev, negativePrompt: e.target.value }))}
              placeholder="What to exclude..."
            />
          </div>

          <div className="playground-form-row">
            <div className="playground-form-group">
              <label>Seed</label>
              <input
                data-testid="playground-seed"
                type="number"
                className="input"
                style={{ width: '100%', boxSizing: 'border-box' }}
                value={form.seed}
                onChange={e => setForm(prev => ({ ...prev, seed: e.target.value }))}
                placeholder="Random"
              />
            </div>
            <div className="playground-form-group">
              <label>Size</label>
              {sizeOptions.length > 0 ? (
                <select
                  data-testid="playground-size"
                  className="input"
                  style={{ width: '100%' }}
                  value={form.size}
                  onChange={e => setForm(prev => ({ ...prev, size: e.target.value }))}
                >
                  {sizeOptions.map(s => (
                    <option key={s} value={s}>{s}</option>
                  ))}
                </select>
              ) : (
                <input
                  data-testid="playground-size"
                  type="text"
                  className="input"
                  style={{ width: '100%', boxSizing: 'border-box' }}
                  value={form.size}
                  onChange={e => setForm(prev => ({ ...prev, size: e.target.value }))}
                  placeholder="e.g. 1024x1024"
                />
              )}
            </div>
            <div className="playground-form-group">
              <label>N (1–4)</label>
              <input
                data-testid="playground-n"
                type="number"
                className="input"
                style={{ width: '100%', boxSizing: 'border-box' }}
                min={1}
                max={4}
                value={form.n}
                onChange={e => setForm(prev => ({ ...prev, n: parseInt(e.target.value, 10) || 1 }))}
              />
            </div>
          </div>

          <div className="playground-form-group">
            <div className="toggle-row">
              <input
                data-testid="playground-strict"
                type="checkbox"
                id="strict-toggle"
                checked={form.strict}
                onChange={e => setForm(prev => ({ ...prev, strict: e.target.checked }))}
              />
              <label htmlFor="strict-toggle" style={{ textTransform: 'none', fontSize: 14, letterSpacing: 0 }}>
                Strict mode
              </label>
            </div>
          </div>

          <button
            data-testid="playground-generate"
            className="btn btn-primary"
            style={{ width: '100%' }}
            disabled={loading || !form.prompt.trim()}
            onClick={handleGenerate}
          >
            {loading ? 'Generating...' : 'Generate'}
          </button>
        </div>

        {/* Right column — Output panel */}
        <div className="playground-panel">
          <div className="playground-tabs">
            <button
              data-testid="playground-tab-image"
              className={`playground-tab${activeTab === 'image' ? ' active' : ''}`}
              onClick={() => setActiveTab('image')}
            >
              Image
            </button>
            <button
              data-testid="playground-tab-request"
              className={`playground-tab${activeTab === 'request' ? ' active' : ''}`}
              onClick={() => setActiveTab('request')}
            >
              Request JSON
            </button>
            <button
              data-testid="playground-tab-response"
              className={`playground-tab${activeTab === 'response' ? ' active' : ''}`}
              onClick={() => setActiveTab('response')}
            >
              Response JSON
            </button>
          </div>

          {activeTab === 'image' && (
            <div className="playground-image-area">
              {imageData?.b64_json ? (
                <img
                  data-testid="playground-image"
                  src={`data:image/png;base64,${imageData.b64_json}`}
                  alt="Generated"
                />
              ) : imageData?.url ? (
                <img
                  data-testid="playground-image"
                  src={imageData.url}
                  alt="Generated"
                />
              ) : (
                <span style={{ color: '#8b949e' }}>No image yet</span>
              )}
            </div>
          )}

          {activeTab === 'request' && (
            <pre
              data-testid="playground-request-json"
              style={{
                background: '#0d1117',
                border: '1px solid #30363d',
                borderRadius: 6,
                padding: 16,
                fontSize: 12,
                color: '#e1e4e8',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
                overflowX: 'auto',
                minHeight: 200,
                margin: 0,
              }}
            >
              {requestJson || '// No request yet'}
            </pre>
          )}

          {activeTab === 'response' && (
            <pre
              data-testid="playground-response-json"
              style={{
                background: '#0d1117',
                border: '1px solid #30363d',
                borderRadius: 6,
                padding: 16,
                fontSize: 12,
                color: '#e1e4e8',
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
                overflowX: 'auto',
                minHeight: 200,
                margin: 0,
              }}
            >
              {responseJson || '// No response yet'}
            </pre>
          )}
        </div>
      </div>

      {/* History strip */}
      {history.length > 0 && (
        <div className="playground-history">
          <h3>History</h3>
          {history.map((entry, i) => (
            <div
              key={entry.id}
              data-testid={`playground-history-row-${i}`}
              className="playground-history-row"
              onClick={() => handleHistoryClick(entry)}
            >
              <div className="history-prompt">{entry.prompt}</div>
              <div className="history-meta">
                <code style={{ fontSize: 11 }}>{entry.model}</code>
                {' · '}
                {new Date(entry.timestamp).toLocaleTimeString()}
              </div>
              <button
                data-testid={`playground-history-resubmit-${i}`}
                className="btn btn-secondary"
                style={{ fontSize: 12, padding: '4px 10px', whiteSpace: 'nowrap' }}
                onClick={e => { e.stopPropagation(); handleResubmit(entry); }}
                title="Re-submit"
              >
                <RotateCcw size={12} />
              </button>
              <button
                data-testid={`playground-history-delete-${i}`}
                className="btn btn-danger"
                style={{ fontSize: 12, padding: '4px 10px' }}
                onClick={e => handleDeleteHistory(entry.id, e)}
                title="Delete"
              >
                <Trash2 size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
