import React, { useEffect, useState } from 'react';
import { client } from '../sdk-client';
import type { ApiKeyInfo, PatchKeyBody, WebhookDelivery } from '@litegen/sdk';
import { showToast } from '../components/toast-store';
import { useTenant } from '../context/tenant';

interface EditState {
  id: string;
  token_quota: string;
  rpm_limit: string;
  scopes: string;
  webhook_url: string;
}

interface WebhookResult {
  keyId: string;
  text: string;
  ok: boolean;
}

export default function Keys() {
  const { activeApp } = useTenant();
  const [keys, setKeys] = useState<ApiKeyInfo[]>([]);
  const [createdKey, setCreatedKey] = useState('');
  const [createdPublicId, setCreatedPublicId] = useState('');
  const [rotatedKey, setRotatedKey] = useState('');
  const [error, setError] = useState('');

  // Create form state
  const [newName, setNewName] = useState('');
  const [newTokenQuota, setNewTokenQuota] = useState('');
  const [newRpmLimit, setNewRpmLimit] = useState('');
  const [newScopes, setNewScopes] = useState('generate,read');

  // Edit state: null = not editing
  const [editState, setEditState] = useState<EditState | null>(null);

  // Webhook test results per key id
  const [webhookResults, setWebhookResults] = useState<Map<string, WebhookResult>>(new Map());

  // Webhook delivery panels: keyId -> { open, deliveries, loading, error }
  interface DeliveryPanel {
    open: boolean;
    deliveries: WebhookDelivery[];
    loading: boolean;
    error: string;
  }
  const [deliveryPanels, setDeliveryPanels] = useState<Map<string, DeliveryPanel>>(new Map());

  const load = () => {
    client.keys.list()
      .then(keys => setKeys(keys))
      .catch(e => setError(e.message));
  };

  // Refetch when the active app changes (the SDK auto-scopes via headers).
  useEffect(() => { load(); }, [activeApp]);

  const createKey = async () => {
    if (!newName.trim()) return;
    setError('');
    try {
      const body = {
        name: newName.trim(),
        scopes: newScopes.trim() || 'generate,read',
        ...(newTokenQuota.trim() ? { token_quota: parseFloat(newTokenQuota) } : {}),
        ...(newRpmLimit.trim() ? { rpm_limit: parseFloat(newRpmLimit) } : {}),
      };

      const result = await client.keys.create(body);
      setCreatedKey(result.key);
      setCreatedPublicId(result.public_id);
      setRotatedKey('');
      setNewName('');
      setNewTokenQuota('');
      setNewRpmLimit('');
      setNewScopes('generate,read');
      load();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  const revokeKey = async (id: string) => {
    setError('');
    try {
      await client.keys.delete(id);
      load();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  const rotateKey = async (id: string) => {
    setError('');
    try {
      const result = await client.keys.rotate(id);
      setRotatedKey(result.key);
      setCreatedKey('');
      setCreatedPublicId('');
      load();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  const copyPrefix = async (_id: string, prefix: string) => {
    try {
      await navigator.clipboard.writeText(prefix);
      showToast('Copied prefix', 'info');
    } catch {
      showToast('Failed to copy', 'error');
    }
  };

  const testWebhook = async (id: string) => {
    try {
      const result = await client.keys.testWebhook(id);
      let text: string;
      let ok: boolean;
      if (result.error) {
        text = `Network error: ${result.error}`;
        ok = false;
      } else if (result.status_code != null) {
        const statusText = result.status_code === 200 ? 'OK'
          : result.status_code === 503 ? 'Service Unavailable'
          : String(result.status_code);
        text = `${result.status_code} ${statusText}`;
        ok = result.delivered;
      } else {
        text = result.delivered ? 'Delivered' : 'Not delivered';
        ok = result.delivered;
      }
      setWebhookResults(prev => new Map(prev).set(id, { keyId: id, text, ok }));
    } catch (e: unknown) {
      const msg = (e as Error).message;
      setWebhookResults(prev => new Map(prev).set(id, { keyId: id, text: `Network error: ${msg}`, ok: false }));
    }
  };

  const startEdit = (key: ApiKeyInfo) => {
    setEditState({
      id: key.id,
      token_quota: key.token_quota != null ? String(key.token_quota) : '',
      rpm_limit: key.rpm_limit != null ? String(key.rpm_limit) : '',
      scopes: key.scopes ?? '',
      webhook_url: key.webhook_url ?? '',
    });
  };

  const cancelEdit = () => setEditState(null);

  const saveEdit = async () => {
    if (!editState) return;
    setError('');
    try {
      const body: PatchKeyBody = {};
      if (editState.token_quota.trim()) {
        body.token_quota = parseFloat(editState.token_quota);
      } else {
        body.token_quota = null;
      }
      if (editState.rpm_limit.trim()) {
        body.rpm_limit = parseFloat(editState.rpm_limit);
      } else {
        body.rpm_limit = null;
      }
      if (editState.scopes.trim()) {
        body.scopes = editState.scopes.trim();
      }
      if (editState.webhook_url.trim()) {
        body.webhook_url = editState.webhook_url.trim();
      } else {
        body.webhook_url = null;
      }
      await client.keys.patch(editState.id, body);
      setEditState(null);
      load();
    } catch (e: unknown) {
      setError((e as Error).message);
    }
  };

  const toggleDeliveries = async (id: string) => {
    const panel = deliveryPanels.get(id);
    if (panel?.open) {
      // Close
      setDeliveryPanels(prev => {
        const next = new Map(prev);
        next.set(id, { ...panel, open: false });
        return next;
      });
      return;
    }
    // Open and fetch
    setDeliveryPanels(prev => new Map(prev).set(id, { open: true, deliveries: [], loading: true, error: '' }));
    try {
      const result = await client.keys.listWebhookDeliveries(id, { page: 1, per_page: 20 });
      setDeliveryPanels(prev => {
        const next = new Map(prev);
        next.set(id, { open: true, deliveries: result.data, loading: false, error: '' });
        return next;
      });
    } catch (e: unknown) {
      setDeliveryPanels(prev => {
        const next = new Map(prev);
        next.set(id, { open: true, deliveries: [], loading: false, error: (e as Error).message });
        return next;
      });
    }
  };

  const formatQuota = (key: ApiKeyInfo) => {
    if (key.token_quota == null) return '—';
    const used = key.tokens_used ?? 0;
    return `${used}/${key.token_quota}`;
  };

  return (
    <div>
      <h2 className="page-title">API Keys</h2>

      {error && <div className="alert alert-error" data-testid="keys-error">{error}</div>}
      {createdKey && (
        <div className="alert alert-success" data-testid="key-created-banner">
          {createdPublicId && (
            <>
              Public key id: <code data-testid="key-public-id" style={{ userSelect: 'all' }}>{createdPublicId}</code>
              <br />
            </>
          )}
          Secret key: <code data-testid="key-secret" style={{ userSelect: 'all' }}>{createdKey}</code>
          <br />
          <small>Copy the secret now — it won't be shown again.</small>
        </div>
      )}
      {rotatedKey && (
        <div className="alert alert-success" data-testid="key-rotate-result-banner">
          Key rotated — new secret: <code style={{ userSelect: 'all' }}>{rotatedKey}</code>
          <br />
          <small>Copy this now — it won't be shown again.</small>
        </div>
      )}

      {/* Create Key Form */}
      <div
        style={{
          background: '#161b22',
          border: '1px solid #30363d',
          borderRadius: 8,
          padding: 16,
          marginBottom: 16,
          display: 'grid',
          gridTemplateColumns: '2fr 1fr 1fr 2fr',
          gap: 8,
          alignItems: 'end',
        }}
      >
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Name *</label>
          <input
            className="input"
            data-testid="new-key-name"
            placeholder="Key name (e.g. 'production')"
            value={newName}
            onChange={e => setNewName(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && createKey()}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Token Quota</label>
          <input
            className="input"
            data-testid="new-key-token-quota"
            type="number"
            placeholder="Unlimited"
            value={newTokenQuota}
            onChange={e => setNewTokenQuota(e.target.value)}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>RPM Limit</label>
          <input
            className="input"
            data-testid="new-key-rpm-limit"
            type="number"
            placeholder="Unlimited"
            value={newRpmLimit}
            onChange={e => setNewRpmLimit(e.target.value)}
          />
        </div>
        <div>
          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Scopes (comma-separated)</label>
          <input
            className="input"
            data-testid="new-key-scopes"
            placeholder="generate,read"
            value={newScopes}
            onChange={e => setNewScopes(e.target.value)}
          />
        </div>
        <button
          className="btn btn-primary"
          data-testid="create-key-btn"
          onClick={createKey}
          style={{ gridColumn: '1 / -1', justifySelf: 'start' }}
        >
          Create Key
        </button>
      </div>

      <div className="table-container">
        <table>
          <thead>
            <tr>
              <th>Name</th>
              <th>Prefix</th>
              <th>Scopes</th>
              <th>Quota (used/total)</th>
              <th>RPM</th>
              <th>Status</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {keys.map(k => (
              <React.Fragment key={k.id}>
                <tr data-testid={`key-row-${k.id}`}>
                  <td data-testid={`key-name-${k.id}`}>{k.name}</td>
                  <td><code data-testid={`key-public-id-${k.id}`}>{k.public_id ?? `${k.prefix}...`}</code></td>
                  <td data-testid={`key-scopes-${k.id}`}>{k.scopes || '—'}</td>
                  <td data-testid={`key-quota-${k.id}`}>{formatQuota(k)}</td>
                  <td data-testid={`key-rpm-${k.id}`}>{k.rpm_limit ?? '—'}</td>
                  <td>
                    <span
                      className={`badge ${k.is_active ? 'healthy' : 'unhealthy'}`}
                      data-testid={`key-status-${k.id}`}
                    >
                      {k.is_active ? 'active' : 'revoked'}
                    </span>
                  </td>
                  <td style={{ display: 'flex', gap: 6, flexWrap: 'wrap', alignItems: 'center' }}>
                    {/* Copy prefix — always available */}
                    <button
                      className="btn btn-secondary"
                      data-testid={`key-copy-prefix-${k.id}`}
                      onClick={() => copyPrefix(k.id, k.prefix)}
                      title="Copy prefix"
                      style={{ fontSize: 12, padding: '4px 8px' }}
                    >
                      Copy
                    </button>
                    {/* View deliveries — always available */}
                    <button
                      className="btn btn-secondary"
                      data-testid={`key-deliveries-${k.id}`}
                      onClick={() => toggleDeliveries(k.id)}
                      title="View webhook deliveries"
                      style={{ fontSize: 12, padding: '4px 8px' }}
                    >
                      Deliveries
                    </button>

                    {k.is_active && (
                      <>
                        <button
                          className="btn btn-secondary"
                          data-testid={`edit-key-${k.id}`}
                          onClick={() => startEdit(k)}
                        >
                          Edit
                        </button>
                        <button
                          className="btn btn-secondary"
                          data-testid={`key-rotate-${k.id}`}
                          onClick={() => rotateKey(k.id)}
                          title="Rotate key"
                          style={{ fontSize: 12, padding: '4px 8px' }}
                        >
                          Rotate
                        </button>
                        <button
                          className="btn btn-danger"
                          data-testid={`revoke-key-${k.id}`}
                          onClick={() => revokeKey(k.id)}
                        >
                          Revoke
                        </button>
                        {k.webhook_url && (
                          <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                            <button
                              className="btn btn-secondary"
                              data-testid={`key-test-webhook-${k.id}`}
                              onClick={() => testWebhook(k.id)}
                              title="Test webhook"
                              style={{ fontSize: 12, padding: '4px 8px' }}
                            >
                              Test Hook
                            </button>
                            {webhookResults.has(k.id) && (
                              <span
                                data-testid={`key-test-webhook-result-${k.id}`}
                                style={{
                                  fontSize: 12,
                                  color: webhookResults.get(k.id)!.ok ? '#3fb950' : '#f85149',
                                  fontFamily: 'monospace',
                                }}
                              >
                                {webhookResults.get(k.id)!.text}
                              </span>
                            )}
                          </span>
                        )}
                      </>
                    )}
                  </td>
                </tr>
                {deliveryPanels.get(k.id)?.open && (
                  <tr key={`deliveries-${k.id}`}>
                    <td colSpan={7}>
                      <div
                        data-testid={`key-deliveries-panel-${k.id}`}
                        style={{
                          background: '#0d1117',
                          border: '1px solid #30363d',
                          borderRadius: 6,
                          padding: 12,
                          fontSize: 12,
                        }}
                      >
                        <strong style={{ color: '#e6edf3', marginBottom: 8, display: 'block' }}>
                          Webhook Deliveries
                        </strong>
                        {deliveryPanels.get(k.id)?.loading && (
                          <span style={{ color: '#8b949e' }}>Loading…</span>
                        )}
                        {deliveryPanels.get(k.id)?.error && (
                          <span style={{ color: '#f85149' }}>{deliveryPanels.get(k.id)?.error}</span>
                        )}
                        {!deliveryPanels.get(k.id)?.loading && !deliveryPanels.get(k.id)?.error && (
                          deliveryPanels.get(k.id)!.deliveries.length === 0 ? (
                            <span style={{ color: '#8b949e' }}>No deliveries recorded yet.</span>
                          ) : (
                            <table style={{ width: '100%', borderCollapse: 'collapse' }}>
                              <thead>
                                <tr>
                                  <th style={{ textAlign: 'left', padding: '4px 8px', color: '#8b949e', fontWeight: 500 }}>Timestamp</th>
                                  <th style={{ textAlign: 'left', padding: '4px 8px', color: '#8b949e', fontWeight: 500 }}>Attempt</th>
                                  <th style={{ textAlign: 'left', padding: '4px 8px', color: '#8b949e', fontWeight: 500 }}>Status</th>
                                  <th style={{ textAlign: 'left', padding: '4px 8px', color: '#8b949e', fontWeight: 500 }}>Success</th>
                                  <th style={{ textAlign: 'left', padding: '4px 8px', color: '#8b949e', fontWeight: 500 }}>Response</th>
                                </tr>
                              </thead>
                              <tbody>
                                {deliveryPanels.get(k.id)!.deliveries.map(d => (
                                  <tr key={d.id} data-testid={`delivery-row-${d.id}`}>
                                    <td style={{ padding: '4px 8px', color: '#e6edf3', whiteSpace: 'nowrap' }}>
                                      {new Date(d.created_at).toLocaleString()}
                                    </td>
                                    <td style={{ padding: '4px 8px', color: '#e6edf3' }}>#{d.attempt_number}</td>
                                    <td style={{ padding: '4px 8px' }}>
                                      {d.status_code != null ? (
                                        <span
                                          style={{
                                            color: d.status_code >= 200 && d.status_code < 300 ? '#3fb950' : '#f85149',
                                            fontFamily: 'monospace',
                                          }}
                                        >
                                          {d.status_code}
                                        </span>
                                      ) : (
                                        <span style={{ color: '#8b949e' }}>—</span>
                                      )}
                                    </td>
                                    <td style={{ padding: '4px 8px' }}>
                                      <span
                                        className={`badge ${d.success ? 'healthy' : 'unhealthy'}`}
                                        style={{ fontSize: 11 }}
                                      >
                                        {d.success ? 'success' : 'failed'}
                                      </span>
                                    </td>
                                    <td style={{ padding: '4px 8px', color: '#8b949e', maxWidth: 200, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                                      {d.response_body
                                        ? d.response_body.slice(0, 80) + (d.response_body.length > 80 ? '…' : '')
                                        : d.error_message ?? '—'}
                                    </td>
                                  </tr>
                                ))}
                              </tbody>
                            </table>
                          )
                        )}
                      </div>
                    </td>
                  </tr>
                )}
                {editState?.id === k.id && (
                  <tr key={`edit-${k.id}`} data-testid={`edit-row-${k.id}`}>
                    <td colSpan={7}>
                      <div
                        style={{
                          background: '#0d1117',
                          border: '1px solid #30363d',
                          borderRadius: 6,
                          padding: 12,
                          display: 'grid',
                          gridTemplateColumns: '1fr 1fr 2fr 2fr',
                          gap: 8,
                          alignItems: 'end',
                        }}
                      >
                        <div>
                          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Token Quota</label>
                          <input
                            className="input"
                            data-testid="edit-token-quota"
                            type="number"
                            placeholder="Unlimited"
                            value={editState.token_quota}
                            onChange={e => setEditState({ ...editState, token_quota: e.target.value })}
                          />
                        </div>
                        <div>
                          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>RPM Limit</label>
                          <input
                            className="input"
                            data-testid="edit-rpm-limit"
                            type="number"
                            placeholder="Unlimited"
                            value={editState.rpm_limit}
                            onChange={e => setEditState({ ...editState, rpm_limit: e.target.value })}
                          />
                        </div>
                        <div>
                          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Scopes</label>
                          <input
                            className="input"
                            data-testid="edit-scopes"
                            placeholder="generate,read"
                            value={editState.scopes}
                            onChange={e => setEditState({ ...editState, scopes: e.target.value })}
                          />
                        </div>
                        <div>
                          <label style={{ display: 'block', marginBottom: 4, color: '#8b949e', fontSize: 12 }}>Webhook URL</label>
                          <input
                            className="input"
                            data-testid="edit-webhook-url"
                            placeholder="https://..."
                            value={editState.webhook_url}
                            onChange={e => setEditState({ ...editState, webhook_url: e.target.value })}
                          />
                        </div>
                        <div style={{ gridColumn: '1 / -1', display: 'flex', gap: 8 }}>
                          <button
                            className="btn btn-primary"
                            data-testid="save-edit-btn"
                            onClick={saveEdit}
                          >
                            Save
                          </button>
                          <button className="btn btn-secondary" onClick={cancelEdit}>Cancel</button>
                        </div>
                      </div>
                    </td>
                  </tr>
                )}
              </React.Fragment>
            ))}
            {keys.length === 0 && (
              <tr>
                <td colSpan={7} style={{ textAlign: 'center', color: '#8b949e' }} data-testid="no-keys-msg">
                  No API keys created yet
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
