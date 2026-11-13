import { useCallback, useEffect, useState } from 'react';
import type { AdminModelView, GatewaySettingsView, ModelSchema, ProviderConfigView, RoutesDocument } from '@litegen/sdk';
import { client } from '../sdk-client';
import { showToast } from '../components/toast-store';
import { accessBody, describeAccess, type AccessForm } from '../components/access';
import { runtimeBody, runtimeFormOf, settingsBody, type RuntimeForm } from '../components/gateway-settings';
import { formatPrice } from '../components/model-detail';
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

/** A byte count in MiB: `256 MiB`, `1.5 MiB`. */
const mib = (bytes: number) => `${(bytes / 1_048_576).toLocaleString('en-US', { maximumFractionDigits: 1 })} MiB`;

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
                  <td style={{ whiteSpace: 'nowrap' }}>{formatPrice(m.schema.pricing.base_cost_usd)}</td>
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
  // Who may open stored files, for apps with no setting of their own.
  const [access, setAccess] = useState<AccessForm>({ mode: '', ownerScope: '', ttl: '', keepOutputs: '', retentionDays: '', signedLinks: '' });
  const [revokedAt, setRevokedAt] = useState('');
  // The cache, retries, the vendor timeout and the breaker (docs/gateway-settings.md).
  const [runtime, setRuntime] = useState<RuntimeForm>(() => runtimeFormOf({}));
  const [error, setError] = useState('');

  const apply = (v: GatewaySettingsView) => {
    setView(v);
    setMarkup(v.settings.cost_markup_percent?.toString() ?? '');
    setWebhook(v.settings.alert_webhook_url ?? '');
    setEuBudget(v.settings.default_end_user_max_budget_usd?.toString() ?? '');
    setEuDuration(v.settings.default_end_user_budget_duration ?? '');
    const keep = v.settings.storage_access_keep_outputs;
    setAccess({
      mode: v.settings.storage_access_mode ?? '',
      ownerScope: v.settings.storage_access_owner_scope ?? '',
      ttl: v.settings.storage_access_signed_url_ttl_seconds?.toString() ?? '',
      keepOutputs: keep === undefined || keep === null ? '' : String(keep),
      retentionDays: v.settings.storage_access_retention_days?.toString() ?? '',
      signedLinks: v.settings.storage_access_signed_links ?? '',
    });
    setRuntime(runtimeFormOf(v.settings));
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
      const { body: files, error: filesError } = accessBody(access);
      if (!files) throw new Error(filesError);
      const { body: runtimeFields, error: runtimeError } = runtimeBody(runtime);
      if (!runtimeFields) throw new Error(runtimeError);
      // Every field is sent, or the save would clear it: the form's over what
      // the server gave, so a setting the form has no field for is kept.
      apply(await client.admin.settings.update(settingsBody(view?.settings ?? {}, {
        cost_markup_percent: num(markup, 'Cost markup'),
        alert_webhook_url: webhook.trim() || null,
        default_end_user_max_budget_usd: num(euBudget, 'Default end-user budget'),
        default_end_user_budget_duration: euDuration.trim() || null,
        storage_access_mode: files.mode ?? null,
        storage_access_owner_scope: files.owner_scope ?? null,
        storage_access_signed_url_ttl_seconds: files.signed_url_ttl_seconds ?? null,
        storage_access_keep_outputs: files.keep_outputs ?? null,
        storage_access_retention_days: files.retention_days ?? null,
        storage_access_signed_links: files.signed_links ?? null,
        ...runtimeFields,
      })));
      showToast('Settings saved', 'info');
    } catch (e) {
      setError(errText(e));
    }
  };

  const clearCache = async () => {
    setError('');
    try {
      await client.cache.clear();
      apply(await client.admin.settings.get());
      showToast('Cache cleared', 'info');
    } catch (e) {
      setError(errText(e));
    }
  };

  const field = (key: keyof RuntimeForm) => ({
    value: runtime[key],
    onChange: (e: { target: { value: string } }) => setRuntime({ ...runtime, [key]: e.target.value }),
  });

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
      <h4 style={{ margin: '16px 0 4px' }}>Stored files</h4>
      <p className="mg-muted" data-testid="gateway-effective-access">
        Apps with no setting of their own: {view ? describeAccess(view.effective_storage_access) : '…'}
      </p>
      <div className="mg-form-grid">
        <label>Links
          <select className="input" value={access.mode} data-testid="gateway-access-mode" onChange={e => setAccess({ ...access, mode: e.target.value })}>
            <option value="">Config default</option>
            <option value="public">Public: the stored URL itself</option>
            <option value="signed">Signed: links that expire</option>
            <option value="private">Private: litegen's file route, with a key</option>
          </select>
        </label>
        <label>Who may open a file through litegen
          <select className="input" value={access.ownerScope} data-testid="gateway-access-scope" onChange={e => setAccess({ ...access, ownerScope: e.target.value })}>
            <option value="">Config default</option>
            <option value="key">The key that made it</option>
            <option value="app">Any key of its app</option>
            <option value="org">Any key of its organization</option>
          </select>
        </label>
        <label>Signed link life (seconds)<input className="input" value={access.ttl} placeholder="config default" data-testid="gateway-access-ttl" onChange={e => setAccess({ ...access, ttl: e.target.value })} /></label>
        <label>Who signs a signed link
          <select className="input" value={access.signedLinks} data-testid="gateway-access-signed-links" onChange={e => setAccess({ ...access, signedLinks: e.target.value })}>
            <option value="">Config default</option>
            <option value="store">The store that holds the file</option>
            <option value="litegen">litegen, for every file (revocable)</option>
          </select>
        </label>
        <label>Keep outputs
          <select className="input" value={access.keepOutputs} data-testid="gateway-access-keep" onChange={e => setAccess({ ...access, keepOutputs: e.target.value })}>
            <option value="">Config default</option>
            <option value="true">Keep a copy of every output</option>
            <option value="false">Keep nothing: the vendor's own links</option>
          </select>
        </label>
        <label>Delete kept files after (days)<input className="input" value={access.retentionDays} placeholder="config default; 0 keeps them" data-testid="gateway-access-retention" onChange={e => setAccess({ ...access, retentionDays: e.target.value })} /></label>
      </div>
      <p className="mg-muted" style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button className="btn btn-danger" data-testid="gateway-revoke-links" onClick={async () => {
          try {
            const r = await client.admin.revokeLinks();
            setRevokedAt(r.revoked_at);
          } catch (e) {
            setError(errText(e));
          }
        }}>Revoke all litegen links</button>
        {revokedAt && <span data-testid="gateway-revoked-at">Every litegen link issued before {new Date(revokedAt).toLocaleString()} is refused.</span>}
      </p>
      <h4 style={{ margin: '16px 0 4px' }}>Cache</h4>
      <p className="mg-muted" data-testid="gateway-cache-effective">
        {view
          ? `${view.effective_cache.enabled ? 'on' : 'off'} · answers kept ${view.effective_cache.ttl_seconds} s · up to ${view.effective_cache.max_items} in ${mib(view.effective_cache.max_bytes)} · ${view.cache_entries} cached now`
          : '…'}
      </p>
      <div className="mg-form-grid">
        <label>Image answers cached
          <select className="input" data-testid="gateway-cache-enabled" {...field('cacheEnabled')}>
            <option value="">Config default</option>
            <option value="on">On</option>
            <option value="off">Off</option>
          </select>
        </label>
        <label>Kept for (seconds)<input className="input" placeholder="config default" data-testid="gateway-cache-ttl" {...field('cacheTtl')} /></label>
        <label>At most (answers)<input className="input" placeholder="config default" data-testid="gateway-cache-size" {...field('cacheMaxItems')} /></label>
        <label>Memory, each replica (bytes)<input className="input" placeholder="config default" data-testid="gateway-cache-max-bytes" {...field('cacheMaxBytes')} /></label>
      </div>
      <div className="mg-actions"><button className="btn btn-secondary" data-testid="gateway-cache-clear" onClick={() => void clearCache()}>Clear cache</button></div>
      <h4 style={{ margin: '16px 0 4px' }}>Retries and timeouts</h4>
      <p className="mg-muted" data-testid="gateway-retry-effective">
        {view
          ? `${view.effective_retries.max_retries} ${view.effective_retries.max_retries === 1 ? 'retry' : 'retries'} · waits up to ${view.effective_retries.backoff_ms} ms, doubling, at most ${view.effective_retries.max_backoff_ms} ms · vendor calls end after ${view.effective_vendor_timeout_seconds} s`
          : '…'}
      </p>
      <div className="mg-form-grid">
        <label>Retries<input className="input" placeholder="config default" data-testid="gateway-retries" {...field('retries')} /></label>
        <label>First backoff (ms)<input className="input" placeholder="config default" data-testid="gateway-backoff" {...field('backoffMs')} /></label>
        <label>Longest backoff (ms)<input className="input" placeholder="config default" data-testid="gateway-max-backoff" {...field('maxBackoffMs')} /></label>
        <label>Vendor timeout (seconds)<input className="input" placeholder="config default" data-testid="gateway-vendor-timeout" {...field('vendorTimeout')} /></label>
      </div>
      <h4 style={{ margin: '16px 0 4px' }}>Circuit breaker</h4>
      <p className="mg-muted" data-testid="gateway-breaker-effective">
        {view
          ? `opens after ${view.effective_circuit_breaker.threshold} failures in a row, for ${view.effective_circuit_breaker.open_for_seconds} s`
          : '…'}
      </p>
      <div className="mg-form-grid">
        <label>Failures to open<input className="input" placeholder="config default" data-testid="gateway-breaker-threshold" {...field('breakerThreshold')} /></label>
        <label>Open for (seconds)<input className="input" placeholder="config default" data-testid="gateway-breaker-open-for" {...field('breakerOpenFor')} /></label>
      </div>
      <div className="mg-actions"><button className="btn btn-primary" data-testid="gateway-settings-save" onClick={() => void save()}>Save settings</button></div>
    </div>
  );
}
