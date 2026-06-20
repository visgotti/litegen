import { BrowserRouter, Routes, Route, NavLink } from 'react-router-dom';
import { BarChart3, Activity, Layers, Database, Key, Plug, Sparkles, Film, ShieldAlert, Users as UsersIcon, Building2 } from 'lucide-react';
import { useState, useEffect } from 'react';
import Overview from './pages/Overview';
import Logs from './pages/Logs';
import Models from './pages/Models';
import Health from './pages/Health';
import Keys from './pages/Keys';
import Playground from './pages/Playground';
import Generations from './pages/Generations';
import Audit from './pages/Audit';
import Login from './pages/Login';
import Signup from './pages/Signup';
import AcceptInvite from './pages/AcceptInvite';
import Account from './pages/Account';
import Users from './pages/Users';
import Organization from './pages/Organization';
import Providers from './pages/Providers';
import Members from './pages/Members';
import UserMenu from './components/UserMenu';
import RequirePermission from './components/RequirePermission';
import RequireAuth from './components/RequireAuth';
import ToastContainer from './components/Toast';
import { TenantProvider } from './context/TenantContext';
import { client, getApiKey } from './sdk-client';
import './App.css';

const BASE_NAV_ITEMS = [
  { to: '/', icon: BarChart3, label: 'Overview', testid: undefined as string | undefined },
  { to: '/logs', icon: Activity, label: 'Logs', testid: undefined },
  { to: '/models', icon: Layers, label: 'Models', testid: undefined },
  { to: '/health', icon: Database, label: 'Health', testid: undefined },
  { to: '/providers', icon: Plug, label: 'Providers', testid: 'nav-providers' },
  { to: '/keys', icon: Key, label: 'API Keys', testid: undefined },
  { to: '/playground', icon: Sparkles, label: 'Playground', testid: undefined },
  { to: '/generations', icon: Film, label: 'Generations', testid: undefined },
  { to: '/audit', icon: ShieldAlert, label: 'Audit', testid: undefined },
  { to: '/organization', icon: Building2, label: 'Organization', testid: 'nav-organization' },
  { to: '/members', icon: UsersIcon, label: 'Members', testid: 'nav-members' },
];

function useMe() {
  const [role, setRole] = useState<string | null>(null);
  // hosted (multi-tenant) mode. The global Users admin page is single-tenant
  // only; in hosted mode members are managed per-org via the Members page.
  // `null` = not yet resolved: we only expose the Users route/link once we have
  // POSITIVELY confirmed single-tenant (hosted === false), so it never flashes
  // in hosted mode before config() resolves and stays hidden if config() fails.
  const [hosted, setHosted] = useState<boolean | null>(null);

  useEffect(() => {
    // signup_open is true only in hosted mode.
    client.auth.config()
      .then(cfg => setHosted(!!(cfg as { signup_open?: boolean }).signup_open))
      .catch(() => setHosted(null));

    // Don't call me() if using the API key flow — it would return 401 and clear the key.
    if (getApiKey()) return;
    client.auth.me()
      .then(resp => {
        const r = resp as { user?: { role?: string } };
        setRole(r?.user?.role ?? null);
      })
      .catch(() => setRole(null));
  }, []);

  return { role, hosted };
}

/** The authenticated application shell: sidebar + header + the app routes. */
function AppShell() {
  const { role, hosted } = useMe();

  // The global Users page operates on the platform-wide users table and is
  // restricted to single-tenant deployments (the backend rejects tenant
  // principals in hosted mode). Hosted tenants use the per-org Members page.
  const canSeeUsers = (role === 'owner' || role === 'admin') && hosted === false;

  const navItems = [
    ...BASE_NAV_ITEMS,
    ...(canSeeUsers ? [{ to: '/users', icon: UsersIcon, label: 'Users', testid: undefined }] : []),
  ];

  return (
    <div className="app">
      <nav className="sidebar">
        <div className="sidebar-header">
          <h1>⚡ LiteGen</h1>
          <span className="subtitle">Proxy Dashboard</span>
        </div>
        <ul className="nav-list">
          {navItems.map(({ to, icon: Icon, label, testid }) => (
            <li key={to}>
              <NavLink
                to={to}
                end={to === '/'}
                data-testid={testid}
                className={({ isActive }) => isActive ? 'active' : ''}
              >
                <Icon size={18} />
                <span>{label}</span>
              </NavLink>
            </li>
          ))}
        </ul>
      </nav>
      <main className="content">
        <UserMenu />
        <Routes>
          <Route path="/" element={<Overview />} />
          <Route path="/logs" element={<Logs />} />
          <Route path="/models" element={<Models />} />
          <Route path="/health" element={<Health />} />
          <Route path="/keys" element={<Keys />} />
          <Route path="/playground" element={<Playground />} />
          <Route path="/generations" element={<Generations />} />
          <Route path="/audit" element={<Audit />} />
          <Route path="/account" element={<Account />} />
          <Route path="/providers" element={<Providers />} />
          <Route path="/organization" element={<Organization />} />
          <Route path="/members" element={<Members />} />
          {/* The global Users page exists only in single-tenant mode. The nav
              link is further gated to owner/admin (canSeeUsers); a non-admin who
              navigates here directly still gets RequirePermission's 403. In
              hosted mode the route is absent entirely (members are managed per-org). */}
          {hosted === false && (
            <Route path="/users" element={
              <RequirePermission perm="user:read:any">
                <Users />
              </RequirePermission>
            } />
          )}
        </Routes>
      </main>
    </div>
  );
}

function App() {
  return (
    <BrowserRouter>
      <TenantProvider>
        <ToastContainer />
        <Routes>
          {/* Public full-screen routes — rendered WITHOUT the sidebar/app chrome. */}
          <Route path="/login" element={<Login />} />
          <Route path="/signup" element={<Signup />} />
          <Route path="/invite/:token" element={<AcceptInvite />} />

          {/* Everything else requires a session and renders the full app shell. */}
          <Route path="*" element={
            <RequireAuth>
              <AppShell />
            </RequireAuth>
          } />
        </Routes>
      </TenantProvider>
    </BrowserRouter>
  );
}

export default App;
