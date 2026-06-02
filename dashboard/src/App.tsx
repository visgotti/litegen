import { BrowserRouter, Routes, Route, NavLink } from 'react-router-dom';
import { BarChart3, Activity, Layers, Database, Key, Sparkles, Film, ShieldAlert, Users as UsersIcon } from 'lucide-react';
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
import UserMenu from './components/UserMenu';
import RequirePermission from './components/RequirePermission';
import ToastContainer from './components/Toast';
import { client, getApiKey } from './sdk-client';
import './App.css';

const BASE_NAV_ITEMS = [
  { to: '/', icon: BarChart3, label: 'Overview' },
  { to: '/logs', icon: Activity, label: 'Logs' },
  { to: '/models', icon: Layers, label: 'Models' },
  { to: '/health', icon: Database, label: 'Health' },
  { to: '/keys', icon: Key, label: 'API Keys' },
  { to: '/playground', icon: Sparkles, label: 'Playground' },
  { to: '/generations', icon: Film, label: 'Generations' },
  { to: '/audit', icon: ShieldAlert, label: 'Audit' },
];

function useMe() {
  const [role, setRole] = useState<string | null>(null);

  useEffect(() => {
    // Don't call me() if using the API key flow — it would return 401 and clear the key.
    if (getApiKey()) return;
    client.auth.me()
      .then(resp => {
        const r = resp as { user?: { role?: string } };
        setRole(r?.user?.role ?? null);
      })
      .catch(() => setRole(null));
  }, []);

  return role;
}

function App() {
  const role = useMe();

  // Admin-or-higher roles can see the Users nav item
  const canSeeUsers = role === 'owner' || role === 'admin';

  const navItems = [
    ...BASE_NAV_ITEMS,
    ...(canSeeUsers ? [{ to: '/users', icon: UsersIcon, label: 'Users' }] : []),
  ];

  return (
    <BrowserRouter>
      <div className="app">
        <nav className="sidebar">
          <div className="sidebar-header">
            <h1>⚡ LiteGen</h1>
            <span className="subtitle">Proxy Dashboard</span>
          </div>
          <ul className="nav-list">
            {navItems.map(({ to, icon: Icon, label }) => (
              <li key={to}>
                <NavLink to={to} end={to === '/'} className={({ isActive }) => isActive ? 'active' : ''}>
                  <Icon size={18} />
                  <span>{label}</span>
                </NavLink>
              </li>
            ))}
          </ul>
        </nav>
        <main className="content">
          <ToastContainer />
          <Routes>
            {/* Unauthenticated routes — no UserMenu header */}
            <Route path="/login" element={<Login />} />
            <Route path="/signup" element={<Signup />} />
            <Route path="/invite/:token" element={<AcceptInvite />} />

            {/* Authenticated app routes — with UserMenu header */}
            <Route path="*" element={
              <>
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
                  <Route path="/users" element={
                    <RequirePermission perm="user:read:any">
                      <Users />
                    </RequirePermission>
                  } />
                </Routes>
              </>
            } />
          </Routes>
        </main>
      </div>
    </BrowserRouter>
  );
}

export default App;
