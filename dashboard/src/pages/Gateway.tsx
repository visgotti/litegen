import { useCallback, useEffect, useState } from 'react';
import type { AdminModelView, GatewaySettingsView, ModelSchema, ProviderConfigView, RoutesDocument } from '@litegen/sdk';
import { client } from '../sdk-client';
import { showToast } from '../components/toast-store';
import '../management.css';

type Tab = 'models' | 'providers' | 'routes' | 'settings';

const MODEL_TEMPLATE = {
  id: 'openai/my-model',
  provider: 'openai',
  media_type: 'image',
  display_name: 'My model',
  description: 'Added at runtime',
  pricing: { base_cost_usd: 0.04 },
  capabilities: { text_to_image: true },
  prompt: { required: true, max_length: 4000 },
};

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Gateway admin: runtime models, provider configs, model routes and settings. */
export default function Gateway() {
  const [tab, setTab] = useState<Tab>('models');
  return (
    <div>
      <h2 className="page-title">Gateway</h2>
      <p className="mg-hint">Change models, provider keys, routing and pricing at runtime. Changes apply to the next request and survive restarts.</p>
      <div className="mg-tabs">
        {(['models', 'providers', 'routes', 'settings'] as const).map(t => (
          <button key={t} className={tab === t ? 'active' : ''} data-testid={`gateway-tab-${t}`} onClick={() => setTab(t)}>
            {t[0]!.toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>
      {tab === 'models' && <ModelsTab />}
      {tab === 'providers' && <ProvidersTab />}
      {tab === 'routes' && <RoutesTab />}
      {tab === 'settings' && <SettingsTab />}
    </div>
  );
}

function ModelsTab() {
  const [models, setModels] = useState<AdminModelView[]>([]);
  const [filter, setFilter] = useState('');
  const [draft, setDraft] = useState(JSON.stringify(MODEL_TEMPLATE, null, 2));
  const [error, setError] = useState('');

  const load = useCallback(() => {
    client.admin.models.list().then(setModels).catch(e => setError(errText(e)));
  }, []);
  useEffect(() => { load(); }, [load]);

  const act = async (fn: () => Promise<unknown>, done: string) => {
    setError('');
    try {
      await fn();
      showToast(done, 'info');
      load();
    } catch (e) {
      setError(errText(e));
    }
  };

  const create = () => {
    let schema: ModelSchema;
    try {
      schema = JSON.parse(draft) as ModelSchema;
    } catch (e) {
      setError(`Model JSON does not parse: ${errText(e)}`);
      return;
    }
    void act(() => client.admin.models.create(schema), `Model ${schema.id} added`);
  };

  const shown = models.filter(m => !filter || m.id.toLowerCase().includes(filter.toLowerCase()));
  const runtime = models.filter(m => m.source === 'runtime').length;
  const disabled = models.filter(m => m.disabled).length;

  return (
    <>
      {error && <div className="alert alert-error" data-testid="gateway-models-error">{error}</div>}
      <div className="mg-card">
        <div className="mg-card-head"><h3>Add a model</h3></div>
        <p className="mg-hint">
          A full model schema, the same shape as a <code>models/*.yaml</code> row and checked by the same rules. The vendor
          model called is the part of the id after <code>&lt;provider&gt;/</code>.
        </p>
        <textarea className="input mg-code" value={draft} data-testid="gateway-model-json" onChange={e => setDraft(e.target.value)} />
        <div className="mg-actions"><button className="btn btn-primary" data-testid="gateway-model-create" onClick={create}>Add model</button></div>
      </div>
      <div className="mg-card">
        <div className="mg-card-head">
          <h3>Models <span className="mg-muted" data-testid="gateway-model-counts">({models.length} total · {runtime} runtime · {disabled} disabled)</span></h3>
          <input className="input" placeholder="Filter" value={filter} data-testid="gateway-model-filter" onChange={e => setFilter(e.target.value)} />
        </div>
        <div className="table-container">
          <table>
            <thead><tr><th>Id</th><th>Source</th><th>Media</th><th>Base cost</th><th>Status</th><th>Actions</th></tr></thead>
            <tbody>
              {shown.map(m => (
                <tr key={m.id} data-testid={`gateway-model-${m.id}`}>
                  <td className="mg-mono">{m.id}</td>
                  <td><span className="badge">{m.source}</span></td>
                  <td>{m.schema.media_type}</td>
                  <td>${m.schema.pricing.base_cost_usd.toFixed(4)}</td>
                  <td><span className={`badge ${m.disabled ? 'unhealthy' : 'healthy'}`}>{m.disabled ? 'disabled' : 'enabled'}</span></td>
                  <td>
                    <div className="mg-actions" style={{ marginTop: 0 }}>
                      <button className="btn btn-secondary" data-testid={`gateway-model-toggle-${m.id}`}
                        onClick={() => void act(() => client.admin.models.setDisabled(m.id, !m.disabled), `${m.id} ${m.disabled ? 'enabled' : 'disabled'}`)}>
                        {m.disabled ? 'Enable' : 'Disable'}
                      </button>
                      {m.source === 'runtime' && (
                        <button className="btn btn-danger" data-testid={`gateway-model-delete-${m.id}`}
                          onClick={() => void act(() => client.admin.models.delete(m.id), `${m.id} deleted`)}>Delete</button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </>
  );
}

function ProvidersTab() {
  const [providers, setProviders] = useState<ProviderConfigView[]>([]);
  const [editing, setEditing] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState('');
  const [apiBase, setApiBase] = useState('');
  const [enabled, setEnabled] = useState(true);
  const [error, setError] = useState('');

  const load = useCallback(() => {
    client.admin.providers.list().then(setProviders).catch(e => setError(errText(e)));
  }, []);
  useEffect(() => { load(); }, [load]);

  const edit = (p: ProviderConfigView) => {
    setEditing(p.name);
    setApiKey('');
    setApiBase(p.api_base ?? '');
    setEnabled(p.enabled);
  };

  const act = async (fn: () => Promise<unknown>, done: string) => {
    setError('');
    try {
      await fn();
      showToast(done, 'info');
      setEditing(null);
      load();
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <>
      {error && <div className="alert alert-error" data-testid="gateway-providers-error">{error}</div>}
      <p className="mg-hint">Keys are encrypted at rest and only ever shown as their last four characters. Leave the key blank to keep the current one.</p>
      <div className="table-container">
        <table>
          <thead><tr><th>Provider</th><th>Media</th><th>Status</th><th>Source</th><th>Key</th><th>API base</th><th>Actions</th></tr></thead>
          <tbody>
            {providers.map(p => (
              <tr key={p.name} data-testid={`gateway-provider-${p.name}`}>
                <td className="mg-mono">{p.name}</td>
                <td>{p.modalities.join(', ')}</td>
                <td><span className={`badge ${p.active ? 'healthy' : 'unhealthy'}`} data-testid={`gateway-provider-status-${p.name}`}>{p.active ? 'active' : 'inactive'}</span></td>
                <td>{p.source}</td>
                <td className="mg-mono" data-testid={`gateway-provider-hint-${p.name}`}>{p.key_hint ?? '—'}</td>
                <td className="mg-mono">{p.api_base ?? 'default'}</td>
                <td>
                  <div className="mg-actions" style={{ marginTop: 0 }}>
                    <button className="btn btn-secondary" data-testid={`gateway-provider-edit-${p.name}`} onClick={() => edit(p)}>Configure</button>
                    {p.source === 'runtime' && (
                      <button className="btn btn-danger" data-testid={`gateway-provider-reset-${p.name}`}
                        onClick={() => void act(() => client.admin.providers.reset(p.name), `${p.name} reset to env config`)}>Reset</button>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {editing && (
        <div className="mg-card" style={{ marginTop: 16 }} data-testid="gateway-provider-form">
          <div className="mg-card-head"><h3>Configure {editing}</h3></div>
          <div className="mg-form-grid">
            <label>API key<input className="input" type="password" autoComplete="off" placeholder="unchanged" value={apiKey}
              data-testid="gateway-provider-key" onChange={e => setApiKey(e.target.value)} /></label>
            <label>API base<input className="input" placeholder="vendor default" value={apiBase}
              data-testid="gateway-provider-base" onChange={e => setApiBase(e.target.value)} /></label>
            <label style={{ flexDirection: 'row', alignItems: 'center', gap: 8 }}>
              <input type="checkbox" checked={enabled} data-testid="gateway-provider-enabled" onChange={e => setEnabled(e.target.checked)} /> Enabled
            </label>
          </div>
          <div className="mg-actions">
            <button className="btn btn-primary" data-testid="gateway-provider-save" onClick={() => void act(
              () => client.admin.providers.configure(editing, {
                api_key: apiKey.trim() || null,
                api_base: apiBase.trim() || null,
                enabled,
              }),
              `${editing} configured`,
            )}>Save</button>
            <button className="btn btn-secondary" onClick={() => setEditing(null)}>Cancel</button>
          </div>
        </div>
      )}
    </>
  );
}

function RoutesTab() {
  const [doc, setDoc] = useState<RoutesDocument | null>(null);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(() => {
    client.admin.routes.get()
      .then(d => { setDoc(d); setDraft(JSON.stringify(d.routes, null, 2)); })
      .catch(e => setError(errText(e)));
  }, []);
  useEffect(() => { load(); }, [load]);

  const save = async () => {
    setError('');
    try {
      const routes = JSON.parse(draft) as RoutesDocument['routes'];
      const d = await client.admin.routes.replace(routes);
      setDoc(d);
      setDraft(JSON.stringify(d.routes, null, 2));
      showToast('Routes saved', 'info');
    } catch (e) {
      setError(errText(e));
    }
  };

  const reset = async () => {
    setError('');
    try {
      const d = await client.admin.routes.reset();
      setDoc(d);
      setDraft(JSON.stringify(d.routes, null, 2));
      showToast('Routes reset to config', 'info');
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <div className="mg-card">
      <div className="mg-card-head">
        <h3>Model routes</h3>
        <span className="badge" data-testid="gateway-routes-source">{doc?.source ?? '…'}</span>
      </div>
      <p className="mg-hint">
        Fallbacks and load balancing. Each route matches a model id (exact, <code>prefix/*</code> or <code>*</code>) and lists
        deployments; strategy is <code>fallback</code>, <code>weighted_round_robin</code>, <code>lowest_cost</code> or <code>lowest_latency</code>.
        The first matching route wins.
      </p>
      {error && <div className="alert alert-error" data-testid="gateway-routes-error">{error}</div>}
      <textarea className="input mg-code" value={draft} data-testid="gateway-routes-json" onChange={e => setDraft(e.target.value)} />
      <div className="mg-actions">
        <button className="btn btn-primary" data-testid="gateway-routes-save" onClick={() => void save()}>Save routes</button>
        {doc?.source === 'runtime' && <button className="btn btn-secondary" data-testid="gateway-routes-reset" onClick={() => void reset()}>Reset to config</button>}
      </div>
    </div>
  );
}

function SettingsTab() {
  const [view, setView] = useState<GatewaySettingsView | null>(null);
  const [markup, setMarkup] = useState('');
  const [webhook, setWebhook] = useState('');
  const [euBudget, setEuBudget] = useState('');
  const [euDuration, setEuDuration] = useState('');
  const [error, setError] = useState('');

  const apply = (v: GatewaySettingsView) => {
    setView(v);
    setMarkup(v.settings.cost_markup_percent?.toString() ?? '');
    setWebhook(v.settings.alert_webhook_url ?? '');
    setEuBudget(v.settings.default_end_user_max_budget_usd?.toString() ?? '');
    setEuDuration(v.settings.default_end_user_budget_duration ?? '');
  };

  useEffect(() => {
    client.admin.settings.get().then(apply).catch(e => setError(errText(e)));
  }, []);

  const save = async () => {
    setError('');
    // NaN would serialize as null and silently clear the field, so refuse it here.
    const num = (s: string, name: string): number | null => {
      if (s.trim() === '') return null;
      const n = Number(s);
      if (!Number.isFinite(n)) throw new Error(`${name} must be a number`);
      return n;
    };
    try {
      apply(await client.admin.settings.update({
        cost_markup_percent: num(markup, 'Cost markup'),
        alert_webhook_url: webhook.trim() || null,
        default_end_user_max_budget_usd: num(euBudget, 'Default end-user budget'),
        default_end_user_budget_duration: euDuration.trim() || null,
      }));
      showToast('Settings saved', 'info');
    } catch (e) {
      setError(errText(e));
    }
  };

  return (
    <div className="mg-card">
      <div className="mg-card-head">
        <h3>Gateway settings</h3>
        <span className="mg-muted" data-testid="gateway-effective-markup">effective markup {view?.effective_cost_markup_percent ?? 0}%</span>
      </div>
      {error && <div className="alert alert-error" data-testid="gateway-settings-error">{error}</div>}
      <div className="mg-form-grid">
        <label>Cost markup (%)<input className="input" value={markup} placeholder="config value" data-testid="gateway-markup" onChange={e => setMarkup(e.target.value)} /></label>
        <label>Budget alert webhook<input className="input" value={webhook} placeholder="https://…" data-testid="gateway-webhook" onChange={e => setWebhook(e.target.value)} /></label>
        <label>Default end-user budget (USD)<input className="input" value={euBudget} placeholder="none" data-testid="gateway-eu-budget" onChange={e => setEuBudget(e.target.value)} /></label>
        <label>Default end-user window<input className="input" value={euDuration} placeholder="e.g. 30d" data-testid="gateway-eu-duration" onChange={e => setEuDuration(e.target.value)} /></label>
      </div>
      <div className="mg-actions"><button className="btn btn-primary" data-testid="gateway-settings-save" onClick={() => void save()}>Save settings</button></div>
    </div>
  );
}
