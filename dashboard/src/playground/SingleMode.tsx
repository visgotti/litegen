import { useEffect, useRef, useState, useCallback } from 'react';
import { Trash2, RotateCcw } from 'lucide-react';
import { client } from '../sdk-client';
import {
  getPlaygroundHistory,
  pushPlaygroundHistory,
  removePlaygroundHistory,
} from '../playground-history';
import type { PlaygroundHistoryEntry } from '../playground-history';
import type { ModelInfo, Model3dGenerationRequest, Model3dGenerationResponse, Model3dAsset, ParamSpec } from '@litegen/sdk';
import ModelPreview from '../components/ModelPreview';
import ParamField from './ParamField';
import RefImagesField from './RefImagesField';
import ExtraParamsField from './ExtraParamsField';
import { useUnifiedParams } from './useUnifiedParams';
import {
  availabilityOf,
  docsUrlOf,
  extraFromBody,
  paramsFromBody,
  promptLength,
  promptMaxLength,
  promptUsage,
  refsFromBody,
  requestProblems,
  schemaInputs,
  sizeEnumOptions,
  sortModels,
} from './params';
import { inlineRefCount, withoutInlineRefs } from './ref-images';
import type { Availability } from './types';
import { MODEL3D_POLL_TIMEOUT_MS } from './useFanOut';

type Tab = 'image' | 'request' | 'response';

/** Params Single Mode has dedicated controls for — they predate the
 *  schema-driven panel and keep their own test ids — so the panel skips them. */
const DEDICATED = new Set(['seed', 'negative_prompt', 'size']);

/** How the model list is grouped. Live models cost money; mock ones run
 *  locally for free; setup ones exist in the catalog but their provider has no
 *  key here, so they run and fail (the error says which key is missing). */
const GROUPS: Array<{ availability: Availability; label: string }> = [
  { availability: 'live', label: 'Live' },
  { availability: 'mock', label: 'Mock (free, local)' },
  { availability: 'setup', label: 'Needs provider setup' },
];

/** Extract a mesh asset list from a stored/live response body, if it's one. Used
 *  to tell a 3D result apart from an image one without threading media_type
 *  through history storage. */
function meshAssetsFromResponse(body: Record<string, unknown>): Model3dAsset[] | null {
  const assets = (body as { assets?: unknown }).assets;
  return Array.isArray(assets) ? (assets as Model3dAsset[]) : null;
}

/** Submit a 3D job and poll it to completion, reporting progress as it goes.
 *  Throws (like `client.images.generate` does on failure) so callers can share
 *  one try/catch with the image path — completion without a mesh is treated
 *  as failure per the platform-wide "a completed 3D job always has a mesh" rule. */
async function generateModel3dJob(
  body: Record<string, unknown>,
  onProgress: (p: number) => void,
  onDropped: (params: string[]) => void,
  signal: AbortSignal,
): Promise<Model3dGenerationResponse> {
  // An explicit budget, not the SDK's 5-minute default: a queued vendor job
  // can outlive that while the server job completes and is billed.
  const pollOpts = { intervalMs: 1500, timeoutMs: MODEL3D_POLL_TIMEOUT_MS };
  const job = client.models3d.generate(body as Model3dGenerationRequest, pollOpts);
  const submitted = await job.submitted;
  onDropped(await job.droppedParams);
  onProgress(submitted.progress ?? 0);
  let final = submitted;
  // The signal stops the polling only; the job itself finishes server-side
  // and stays reachable from Generations.
  for await (const update of client.models3d.poll(submitted.id, { ...pollOpts, signal })) {
    final = update;
    onProgress(update.progress ?? 0);
  }
  if (final.status !== 'completed') {
    throw new Error(final.error ?? `job ${final.status}`);
  }
  if (!final.assets?.some(a => a.kind === 'mesh')) {
    throw new Error('completed without a mesh asset');
  }
  return final;
}

export interface FormState {
  model: string;
  prompt: string;
  negativePrompt: string;
  seed: string;
  size: string;
  n: number;
  strict: boolean;
}

/**
 * Build the request body for one generation, sending only the fields the
 * selected media type actually accepts.
 *
 * `size` and `n` are image-only: `Model3dGenerationRequest` has no `size` at
 * all and a 3D job is billed flat per generation regardless of `n`. Serde
 * drops an unknown `size` silently — including under `strict: true`, where the
 * checkbox promises the request would be rejected — so sending them would tell
 * the user their 4 meshes at 1024x1024 were accepted when one default mesh is
 * what comes back. The form disables both controls for a 3D model for the
 * same reason.
 *
 * `negative_prompt` and `seed` go only to a model whose schema (`declared`,
 * its params) lists them: in strict mode either one is otherwise a 400, and a
 * value left over from the previously selected model is the usual way to send
 * one. No 3D model takes a negative prompt. Until the schema has loaded
 * (`declared` undefined) they are sent as entered.
 */
// eslint-disable-next-line react-refresh/only-export-components -- pure helper, exported for its unit test
export function buildGenerationBody(
  form: FormState,
  mediaType: 'image' | 'model3d',
  prompt: 'required' | 'optional' | 'unused' = 'required',
  declared?: Record<string, unknown>,
): Record<string, unknown> {
  const isModel3d = mediaType === 'model3d';
  const body: Record<string, unknown> = { model: form.model };
  // A model that takes no prompt (`prompt.max_length == 0`) must not be sent
  // one — strict mode rejects it — even if one is left over from the model
  // selected before. A blank prompt is simply omitted: the field is optional.
  if (prompt !== 'unused' && form.prompt.trim() !== '') body.prompt = form.prompt;
  if (!isModel3d) body.n = form.n;
  body.strict = form.strict;
  const takes = (name: string) => declared === undefined || name in declared;
  if (form.negativePrompt && takes('negative_prompt')) body.negative_prompt = form.negativePrompt;
  if (form.seed && takes('seed')) body.seed = parseInt(form.seed, 10);
  if (!isModel3d && form.size) body.size = form.size;
  return body;
}

/** Same rule as `buildGenerationBody`, applied to a stored history body: an
 *  entry recorded against an image model (or before 3D dropped these fields)
 *  must not resubmit `size`/`n` to the 3D endpoint, which ignores them. */
// eslint-disable-next-line react-refresh/only-export-components -- pure helper, exported for its unit test
export function omitImageOnlyParams(body: Record<string, unknown>): Record<string, unknown> {
  const rest = { ...body };
  delete rest.size;
  delete rest.n;
  return rest;
}

/**
 * The size actually sent: the user's pick while the model offers it, else the
 * model's first size, else whatever was typed for a free-form size. Derived
 * rather than stored, so a model switch can never carry a size the new model
 * does not offer — and there is no schema effect racing a stale response.
 */
// eslint-disable-next-line react-refresh/only-export-components -- pure helper, exported for its unit test
export function effectiveSize(chosen: string, options: string[]): string {
  if (options.length === 0) return chosen;
  return options.includes(chosen) ? chosen : options[0];
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
  const [form, setForm] = useState<FormState>(DEFAULT_FORM);
  const [loading, setLoading] = useState(false);
  const [activeTab, setActiveTab] = useState<Tab>('image');
  const [requestJson, setRequestJson] = useState<string>('');
  const [responseJson, setResponseJson] = useState<string>('');
  const [imageData, setImageData] = useState<{ b64_json?: string | null; url?: string | null } | null>(null);
  const [resultMediaType, setResultMediaType] = useState<'image' | 'model3d'>('image');
  const [meshAssets, setMeshAssets] = useState<Model3dAsset[]>([]);
  const [droppedParams, setDroppedParams] = useState<string[]>([]);
  const [progress, setProgress] = useState<number | null>(null);
  const [history, setHistory] = useState<PlaygroundHistoryEntry[]>([]);
  const [error, setError] = useState('');

  // The schema-driven half of the form — params, reference images, provider
  // passthrough — shares Compare Mode's machinery for the one selected model.
  const unified = useUnifiedParams(form.model ? [form.model] : [], DEDICATED);
  const schema = unified.schemasById[form.model];
  const selected = models.find(m => m.id === form.model);
  const isModel3dSelected = selected?.media_type === 'model3d';
  const mediaType: 'image' | 'model3d' = isModel3dSelected ? 'model3d' : 'image';

  const declaredParams = schema ? ((schema.params ?? {}) as Record<string, ParamSpec>) : undefined;
  const takesNegativePrompt = declaredParams === undefined || 'negative_prompt' in declaredParams;
  const takesSeed = declaredParams === undefined || 'seed' in declaredParams;
  const sizeSpec = (schema?.params as Record<string, ParamSpec> | undefined)?.size;
  const sizeOptions = sizeSpec ? sizeEnumOptions(sizeSpec) : [];
  const size = effectiveSize(form.size, sizeOptions);

  // Before the schema arrives, only the prompt can be judged.
  const problems = schema
    ? requestProblems(schema, { prompt: form.prompt, refs: unified.form.refs })
    : form.prompt.trim() ? [] : ['needs a prompt'];

  // Load every image and 3D model on mount, mock and real alike — the full
  // inspector lives here, so every model has to be reachable from it.
  useEffect(() => {
    client.models.list()
      .then(allModels => {
        const usable = sortModels(allModels.filter(m => m.media_type === 'image' || m.media_type === 'model3d'));
        setModels(usable);
        if (usable.length > 0) {
          // Prefer the visual mock model so the Playground shows real visible
          // output without spending anything.
          const fallback = usable.find(m => m.id === 'mock/visual-image-gen')
            ?? usable.find(m => availabilityOf(m) === 'mock')
            ?? usable[0];
          setForm(prev => ({ ...prev, model: fallback.id }));
        }
      })
      .catch(e => setError(e.message));
  }, []);

  // Load history on mount
  useEffect(() => {
    setHistory(getPlaygroundHistory());
  }, []);

  const refreshHistory = useCallback(() => {
    setHistory(getPlaygroundHistory());
  }, []);

  // The run in flight. Leaving Single mode aborts its polling, which would
  // otherwise go on for up to MODEL3D_POLL_TIMEOUT_MS after the panel is gone.
  const runCtrl = useRef<AbortController | null>(null);
  useEffect(() => () => runCtrl.current?.abort(), []);

  /** A model switch starts its params afresh: values chosen for one model's
   *  schema mean nothing (or something else) on another's. Reference images
   *  stay — trying one input image on several models is the common case — and
   *  each model is only sent the roles it declares. */
  const selectModel = (model: string) => {
    setForm(prev => ({ ...prev, model, size: '' }));
    unified.setForm(prev => ({ ...prev, params: {}, extra: {} }));
  };

  const usage = schema ? promptUsage(schema) : 'required';
  const promptMax = promptMaxLength(schema);

  /** The full request: the dedicated fields plus the schema-driven ones. */
  const buildBody = (): Record<string, unknown> => ({
    ...buildGenerationBody({ ...form, size }, mediaType, usage, declaredParams),
    ...schemaInputs(schema, unified.form),
  });

  /** Runs one request for `model` and records it; shared by Generate and Re-submit. */
  const runRequest = async (model: string, prompt: string, body: Record<string, unknown>) => {
    const isModel3d = models.find(m => m.id === model)?.media_type === 'model3d';
    const ctrl = new AbortController();
    runCtrl.current = ctrl;
    setLoading(true);
    setError('');
    setProgress(null);
    setDroppedParams([]);
    setResultMediaType(isModel3d ? 'model3d' : 'image');
    if (isModel3d) setMeshAssets([]); else setImageData(null);
    setRequestJson(JSON.stringify(body, null, 2));

    try {
      let responseBody: Record<string, unknown>;
      if (isModel3d) {
        const final = await generateModel3dJob(body, setProgress, setDroppedParams, ctrl.signal);
        setMeshAssets(final.assets ?? []);
        responseBody = final as unknown as Record<string, unknown>;
      } else {
        // Cast body to ImageGenerationRequest — the server accepts extra fields gracefully.
        const res = await client.images.generate(body as Parameters<typeof client.images.generate>[0]);
        setImageData(res.data?.[0] ?? null);
        responseBody = res as Record<string, unknown>;
      }
      setResponseJson(JSON.stringify(responseBody, null, 2));
      setActiveTab('image');

      pushPlaygroundHistory({
        id: crypto.randomUUID(),
        model,
        prompt,
        timestamp: new Date().toISOString(),
        // Uploaded images are megabytes of base64; localStorage holds ~5 MB.
        requestBody: withoutInlineRefs(body),
        responseBody,
        ...(inlineRefCount(body) > 0 ? { strippedRefs: inlineRefCount(body) } : {}),
      });
      refreshHistory();
    } catch (e: unknown) {
      if (ctrl.signal.aborted) return; // left Single mode; nothing to show
      setError((e as Error).message);
    } finally {
      setLoading(false);
      setProgress(null);
    }
  };

  const handleGenerate = async () => {
    if (problems.length > 0) return;
    await runRequest(form.model, form.prompt, buildBody());
  };

  const handleHistoryClick = (entry: PlaygroundHistoryEntry) => {
    const req = entry.requestBody;
    setForm(prev => ({
      ...prev,
      model: entry.model,
      prompt: entry.prompt,
      negativePrompt: (req.negative_prompt as string) ?? '',
      seed: req.seed != null ? String(req.seed) : '',
      size: (req.size as string) ?? '',
      n: (req.n as number) ?? 1,
      strict: req.strict != null ? (req.strict as boolean) : true,
    }));
    unified.setForm(prev => ({
      ...prev,
      params: paramsFromBody(req),
      refs: refsFromBody(req),
      extra: extraFromBody(req),
    }));
    setRequestJson(JSON.stringify(req, null, 2));
    setResponseJson(JSON.stringify(entry.responseBody, null, 2));
    setDroppedParams([]);
    const assets = meshAssetsFromResponse(entry.responseBody);
    if (assets) {
      setResultMediaType('model3d');
      setMeshAssets(assets);
      setImageData(null);
    } else {
      const respData = entry.responseBody as { data?: Array<{ b64_json?: string; url?: string }> };
      setResultMediaType('image');
      setImageData(respData?.data?.[0] ?? null);
      setMeshAssets([]);
    }
    setActiveTab('image');
  };

  const handleResubmit = async (entry: PlaygroundHistoryEntry) => {
    handleHistoryClick(entry);
    if (entry.strippedRefs) {
      // History keeps no uploaded images (see withoutInlineRefs). Sending the
      // stored body would run without them, and where the image is optional
      // that is a different, billed job: load the entry and ask instead.
      const n = entry.strippedRefs;
      setError(
        `This run used ${n === 1 ? 'an uploaded image' : `${n} uploaded images`}, which history does not keep. `
        + `Add ${n === 1 ? 'it' : 'them'} again, then Generate.`,
      );
      return;
    }
    // Small delay to let state settle before firing
    await new Promise(r => setTimeout(r, 0));
    const isModel3d = models.find(m => m.id === entry.model)?.media_type === 'model3d';
    const body = isModel3d ? omitImageOnlyParams(entry.requestBody) : entry.requestBody;
    await runRequest(entry.model, entry.prompt, body);
  };

  const handleDeleteHistory = (id: string, e: React.MouseEvent) => {
    e.stopPropagation();
    removePlaygroundHistory(id);
    refreshHistory();
  };

  const docsUrl = docsUrlOf(schema);

  return (
    <div>
      {error && <div className="alert alert-error">{error}</div>}

      <div className="playground-layout">
        {/* Left column — Input form */}
        <div className="playground-panel">
          <div className="playground-form-group">
            <label htmlFor="playground-model-input">Model</label>
            <select
              id="playground-model-input"
              data-testid="playground-model"
              className="input"
              style={{ width: '100%' }}
              value={form.model}
              disabled={loading}
              onChange={e => selectModel(e.target.value)}
            >
              {GROUPS.map(g => {
                const inGroup = models.filter(m => availabilityOf(m) === g.availability);
                return inGroup.length ? (
                  <optgroup key={g.availability} label={g.label}>
                    {inGroup.map(m => (
                      <option key={m.id} value={m.id}>{m.id}</option>
                    ))}
                  </optgroup>
                ) : null;
              })}
            </select>
            {selected?.description ? (
              <p
                data-testid="playground-model-description"
                style={{ margin: '4px 0 0', fontSize: 12, color: 'var(--text-muted, #888)', lineHeight: 1.4 }}
              >
                {selected.description}
              </p>
            ) : null}
            {docsUrl && (
              <a href={docsUrl} target="_blank" rel="noreferrer" data-testid="playground-model-docs"
                className="pg-docs-link">
                Provider API reference ↗
              </a>
            )}
          </div>

          {usage === 'unused' ? (
            // Hidden, not just disabled: this model has no prompt to take, and
            // the body never carries one for it (see buildGenerationBody).
            <p className="pg-param-description" data-testid="playground-prompt-unused" style={{ margin: '0 0 12px' }}>
              This model takes no prompt — it generates from the input image.
            </p>
          ) : (
            <div className="playground-form-group">
              <label htmlFor="playground-prompt-input" data-testid="playground-prompt-label">
                Prompt{usage === 'optional' ? ' (optional with a reference image)' : ''}
              </label>
              <textarea
                id="playground-prompt-input"
                data-testid="playground-prompt"
                rows={3}
                value={form.prompt}
                onChange={e => setForm(prev => ({ ...prev, prompt: e.target.value }))}
                placeholder={isModel3dSelected ? 'Describe the 3D model...' : 'Describe the image...'}
              />
              {promptMax != null && (
                <span className="pg-param-description" data-testid="playground-prompt-count">
                  {promptLength(form.prompt).toLocaleString()} / {promptMax.toLocaleString()} characters
                </span>
              )}
            </div>
          )}

          <RefImagesField roles={unified.roles} refs={unified.form.refs} maxTotal={unified.maxRefs}
            totalSelected={1} disabled={loading}
            onChange={refs => unified.setForm(prev => ({ ...prev, refs }))} />

          <div className="playground-form-group">
            <label>Negative Prompt (optional)</label>
            <textarea
              data-testid="playground-negative-prompt"
              rows={2}
              disabled={!takesNegativePrompt}
              title={takesNegativePrompt ? undefined : 'This model takes no negative prompt; it is not sent'}
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
                disabled={!takesSeed}
                title={takesSeed ? undefined : 'This model takes no seed; it is not sent'}
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
                  value={size}
                  disabled={isModel3dSelected}
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
                  value={size}
                  disabled={isModel3dSelected}
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
                disabled={isModel3dSelected}
                onChange={e => setForm(prev => ({ ...prev, n: parseInt(e.target.value, 10) || 1 }))}
              />
            </div>
          </div>

          {/* Disabled rather than removed so the controls stay in place as the
              model changes; the body omits them entirely for 3D. */}
          {isModel3dSelected && (
            <p
              data-testid="playground-3d-param-note"
              style={{ margin: '-4px 0 12px', fontSize: 12, color: 'var(--text-muted, #8b949e)' }}
            >
              Size and N don’t apply to 3D models — one job returns one mesh.
            </p>
          )}

          {unified.merged.length > 0 && (
            <div className="pg-params" data-testid="playground-params">
              {unified.merged.map(p => (
                <ParamField key={p.name} name={p.name} spec={p.spec} models={p.models}
                  totalSelected={1} value={unified.params[p.name]}
                  onChange={v => unified.setParam(p.name, v)}
                  onReset={p.name in unified.form.params ? () => unified.resetParam(p.name) : undefined} />
              ))}
            </div>
          )}

          <ExtraParamsField extras={unified.extras} values={unified.form.extra} totalSelected={1}
            docsUrl={docsUrl} disabled={loading}
            onChange={(key, raw) => unified.setForm(prev => ({ ...prev, extra: { ...prev.extra, [key]: raw } }))} />

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
            disabled={loading || problems.length > 0}
            onClick={handleGenerate}
          >
            {loading ? (progress != null ? `Generating… ${progress}%` : 'Generating...') : 'Generate'}
          </button>
          {problems.length > 0 && !loading && (
            <p className="pg-problems-inline" data-testid="playground-problems">
              This model {problems.join(', ')}.
            </p>
          )}
        </div>

        {/* Right column — Output panel */}
        <div className="playground-panel">
          <div className="playground-tabs">
            <button
              data-testid="playground-tab-image"
              className={`playground-tab${activeTab === 'image' ? ' active' : ''}`}
              onClick={() => setActiveTab('image')}
            >
              {resultMediaType === 'model3d' ? '3D Model' : 'Image'}
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

          {droppedParams.length > 0 && (
            <div className="pg-dropped" data-testid="playground-dropped"
              title="Strict mode was off, so the server dropped these instead of rejecting the request">
              Not supported by this model, so dropped: <code>{droppedParams.join(', ')}</code>
            </div>
          )}

          {activeTab === 'image' && (
            <div className="playground-image-area">
              {resultMediaType === 'model3d' ? (
                meshAssets.length > 0 ? (
                  // The single result has the user's full attention, so it
                  // gets the inspector; Compare-mode tiles stay compact.
                  <ModelPreview assets={meshAssets} testId="playground-mesh" />
                ) : (
                  <span style={{ color: '#8b949e' }}>
                    {progress != null ? `generating… ${progress}%` : 'No mesh yet'}
                  </span>
                )
              ) : imageData?.b64_json ? (
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
              // A run in flight (a 3D poll lasts minutes) owns the panel and the
              // form; loading or starting another would interleave the two.
              aria-disabled={loading}
              style={loading ? { opacity: 0.6, cursor: 'not-allowed' } : undefined}
              onClick={() => { if (!loading) handleHistoryClick(entry); }}
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
                disabled={loading}
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
