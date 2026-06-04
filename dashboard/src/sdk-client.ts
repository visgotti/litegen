import { LiteGenClient } from '@litegen/sdk';
import { showToast } from './components/toast-store';

// ─── API-key helpers (used by AuthBar and legacy code) ──────────────────────

export function getApiKey(): string {
  return localStorage.getItem('litegen_api_key') ?? '';
}

export function setApiKey(key: string): void {
  localStorage.setItem('litegen_api_key', key);
}

export function clearApiKey(): void {
  localStorage.removeItem('litegen_api_key');
}

// API base the SDK talks to. In prod this is e.g. https://app.litegen.ai/api.
// Exported so pages (Login/Signup) can build OAuth start URLs against the same origin.
export const API_BASE = (import.meta.env.VITE_API_URL || 'http://localhost:4000').replace(/\/$/, '');
const BASE = API_BASE;

let csrfCache: { token: string; fetchedAt: number } | null = null;
const CSRF_TTL_MS = 60_000;

async function getCsrfToken(): Promise<string | undefined> {
  if (csrfCache && Date.now() - csrfCache.fetchedAt < CSRF_TTL_MS) {
    return csrfCache.token;
  }
  try {
    const res = await fetch(`${BASE}/v1/auth/csrf`, { credentials: 'include' });
    if (!res.ok) return undefined;
    const json = await res.json();
    csrfCache = { token: json.csrf_token, fetchedAt: Date.now() };
    return json.csrf_token;
  } catch { return undefined; }
}

export function clearCsrfCache() { csrfCache = null; }

export const client = new LiteGenClient({
  baseUrl: BASE,
  // Bearer (API-key) fallback: pulled from localStorage by the master-key flow.
  getAuthToken: () => localStorage.getItem('litegen_api_key') ?? undefined,
  // Session cookie auth: browser auto-includes Cookie header.
  credentials: 'include',
  getCsrfToken,
  onError: (status: number, body: unknown) => {
    if (status === 401) {
      localStorage.removeItem('litegen_api_key');
      csrfCache = null;
      window.dispatchEvent(new Event('litegen:unauthenticated'));
    }
    if ([402, 403, 429].includes(status)) {
      const bodyObj = body as { error?: { message?: string } } | null;
      showToast(bodyObj?.error?.message ?? 'Request denied', 'error');
    }
  },
});

// Initialize the active-tenant headers from localStorage so the very first
// requests (before TenantProvider resolves /auth/me) are already scoped.
{
  const initOrg = localStorage.getItem('litegen_active_org') ?? undefined;
  const initApp = localStorage.getItem('litegen_active_app') ?? undefined;
  if (initOrg || initApp) client.setActiveTenant(initOrg, initApp);
}
