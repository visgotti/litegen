import { useTenant } from '../context/tenant';

const selectStyle: React.CSSProperties = {
  background: '#0d1117',
  border: '1px solid #30363d',
  borderRadius: 6,
  padding: '5px 10px',
  color: '#e6edf3',
  fontSize: 13,
  cursor: 'pointer',
  maxWidth: 180,
};

/**
 * Org + App switcher rendered in the authenticated header. Hidden when the
 * user has no orgs (e.g. API-key flow or single-tenant backend).
 */
export default function OrgSwitcher() {
  const { orgs, activeOrg, activeApp, apps, switchOrg, switchApp } = useTenant();

  if (orgs.length === 0) return null;

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <select
        data-testid="org-switcher"
        value={activeOrg ?? ''}
        onChange={e => { void switchOrg(e.target.value); }}
        style={selectStyle}
        title="Active organization"
      >
        {orgs.map(o => (
          <option key={o.id} value={o.id}>{o.name}</option>
        ))}
      </select>
      <select
        data-testid="app-switcher"
        value={activeApp ?? ''}
        onChange={e => switchApp(e.target.value)}
        style={selectStyle}
        title="Active application"
        disabled={apps.length === 0}
      >
        {apps.length === 0 && <option value="">No apps</option>}
        {apps.map(a => (
          <option key={a.id} value={a.id}>{a.name}</option>
        ))}
      </select>
    </div>
  );
}
