import { useState, useEffect, useRef } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { client, clearCsrfCache, getApiKey } from '../sdk-client';
import AuthBar from './AuthBar';

interface MeUser {
  id?: string;
  email: string;
  role: string;
}

const STORAGE_KEY = 'litegen_use_api_key';

function roleBadgeStyle(role: string) {
  const colors: Record<string, { bg: string; color: string }> = {
    owner: { bg: '#31213a', color: '#d2a8ff' },
    admin: { bg: '#1a3f5c', color: '#58a6ff' },
    member: { bg: '#1a4731', color: '#3fb950' },
    viewer: { bg: '#2d2a1a', color: '#e3b341' },
  };
  const c = colors[role] ?? { bg: '#21262d', color: '#8b949e' };
  return {
    fontSize: 11,
    fontWeight: 600 as const,
    padding: '2px 8px',
    borderRadius: 999,
    background: c.bg,
    color: c.color,
    textTransform: 'capitalize' as const,
  };
}

export default function UserMenu() {
  const navigate = useNavigate();
  const [me, setMe] = useState<MeUser | null>(null);
  const [authChecked, setAuthChecked] = useState(false);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  // useApiKey: explicit override from localStorage.
  // null = no preference stored → auto-show unless we confirm a session exists.
  // We start with null and resolve it after the me() check.
  const [useApiKey, setUseApiKey] = useState<boolean | null>(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    return stored === null ? null : stored === 'true';
  });
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Fetch current session user on mount.
  // Skip if there's already an API key in localStorage (user is in API-key flow).
  useEffect(() => {
    if (getApiKey()) {
      // API-key flow: skip session check, just mark auth checked with no session user.
      setAuthChecked(true);
      return;
    }
    client.auth.me()
      .then((resp) => {
        const r = resp as { user?: MeUser };
        if (r?.user) {
          setMe(r.user);
        }
      })
      .catch(() => {
        // 401 = no session, that's fine
      })
      .finally(() => {
        setAuthChecked(true);
      });
  }, []);

  // Close dropdown when clicking outside
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, []);

  // Listen for unauthenticated events (401 on API calls)
  useEffect(() => {
    const handler = () => {
      setMe(null);
    };
    window.addEventListener('litegen:unauthenticated', handler);
    return () => window.removeEventListener('litegen:unauthenticated', handler);
  }, []);

  const handleSignOut = async () => {
    setDropdownOpen(false);
    try {
      await client.auth.logout();
    } catch {
      // ignore errors on logout
    }
    clearCsrfCache();
    setMe(null);
    navigate('/login');
  };

  const toggleApiKey = () => {
    // When toggling, use current effective value
    const effective = useApiKey !== null ? useApiKey : (!me && authChecked);
    const next = !effective;
    setUseApiKey(next);
    localStorage.setItem(STORAGE_KEY, String(next));
  };

  // Show the API key bar if:
  // 1. User explicitly enabled it, OR
  // 2. No explicit preference AND no session (unauthenticated) — fallback for master-key flow
  const showApiKeyBar = useApiKey !== null
    ? useApiKey
    : (authChecked && !me);

  return (
    <div style={{ background: '#161b22', borderBottom: '1px solid #30363d', marginBottom: 16 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '10px 16px' }}>

        {/* Left: session-user info or sign-in link */}
        {authChecked && me ? (
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, flex: 1 }} ref={dropdownRef}>
            <div style={{ position: 'relative' }}>
              <button
                data-testid="user-menu-toggle"
                onClick={() => setDropdownOpen(o => !o)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  background: 'transparent',
                  border: '1px solid #30363d',
                  borderRadius: 8,
                  padding: '6px 12px',
                  cursor: 'pointer',
                  color: '#e6edf3',
                }}
              >
                <span data-testid="user-menu-email" style={{ fontSize: 13, fontWeight: 500 }}>{me.email}</span>
                <span data-testid="user-menu-role" style={roleBadgeStyle(me.role)}>{me.role}</span>
                <svg width="12" height="12" viewBox="0 0 12 12" fill="currentColor" style={{ color: '#8b949e', flexShrink: 0 }}>
                  <path d="M6 8L1 3h10z"/>
                </svg>
              </button>

              {dropdownOpen && (
                <div style={{
                  position: 'absolute',
                  top: '100%',
                  left: 0,
                  marginTop: 4,
                  width: 180,
                  background: '#161b22',
                  border: '1px solid #30363d',
                  borderRadius: 8,
                  boxShadow: '0 8px 24px rgba(0,0,0,0.4)',
                  zIndex: 100,
                  overflow: 'hidden',
                }}>
                  <Link
                    to="/account"
                    data-testid="user-menu-account"
                    onClick={() => setDropdownOpen(false)}
                    style={{
                      display: 'block',
                      padding: '10px 16px',
                      color: '#e6edf3',
                      textDecoration: 'none',
                      fontSize: 14,
                    }}
                  >
                    Account
                  </Link>
                  <button
                    data-testid="user-menu-signout"
                    onClick={handleSignOut}
                    style={{
                      display: 'block',
                      width: '100%',
                      padding: '10px 16px',
                      background: 'transparent',
                      border: 'none',
                      borderTop: '1px solid #30363d',
                      color: '#f85149',
                      textAlign: 'left',
                      cursor: 'pointer',
                      fontSize: 14,
                    }}
                  >
                    Sign out
                  </button>
                </div>
              )}
            </div>
          </div>
        ) : authChecked ? (
          <div style={{ flex: 1 }}>
            <Link
              to="/login"
              data-testid="user-menu-signin-link"
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 6,
                padding: '6px 14px',
                background: '#1f6feb',
                color: '#fff',
                borderRadius: 6,
                textDecoration: 'none',
                fontSize: 13,
                fontWeight: 500,
              }}
            >
              Sign in
            </Link>
          </div>
        ) : (
          <div style={{ flex: 1 }} />
        )}

        {/* Right: "Use API key instead" toggle */}
        <button
          data-testid="user-menu-use-api-key"
          onClick={toggleApiKey}
          style={{
            background: 'transparent',
            border: '1px solid #30363d',
            borderRadius: 6,
            padding: '4px 10px',
            color: showApiKeyBar ? '#58a6ff' : '#8b949e',
            cursor: 'pointer',
            fontSize: 12,
            whiteSpace: 'nowrap',
          }}
        >
          {showApiKeyBar ? 'Using API key' : 'Use API key instead'}
        </button>
      </div>

      {/* AuthBar: always shown when no session, or when explicitly toggled on */}
      {showApiKeyBar && (
        <div style={{ borderTop: '1px solid #21262d' }}>
          <AuthBar />
        </div>
      )}
    </div>
  );
}
