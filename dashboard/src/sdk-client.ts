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
export const API_BASE = __LITEGEN_API_BASE__;
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

/** A 401 means the session (or key) is gone: sign out everywhere. */
function onUnauthenticated(status: number) {
  if (status !== 401) return;
  localStorage.removeItem('litegen_api_key');
  csrfCache = null;
  window.dispatchEvent(new Event('litegen:unauthenticated'));
}

export const client = new LiteGenClient({
  baseUrl: BASE,
  // Bearer (API-key) fallback: pulled from localStorage by the master-key flow.
  getAuthToken: () => localStorage.getItem('litegen_api_key') ?? undefined,
  // Session cookie auth: browser auto-includes Cookie header.
  credentials: 'include',
  getCsrfToken,
  onError: (status: number, body: unknown) => {
    onUnauthenticated(status);
    if ([402, 403, 429].includes(status)) {
      const bodyObj = body as { error?: { message?: string } } | null;
      showToast(bodyObj?.error?.message ?? 'Request denied', 'error');
    }
  },
});

/**
 * The same API for reads whose refusal the page shows itself, so a 402, 403
 * or 429 raises no toast here: the Playground's pre-submit checks (E11b),
 * which ask a quote route after every edit, and the Health page's provider
 * read, which a hosted tenant is refused. It sends the main client's active
 * org and app with every request.
 */
export const quietClient = new LiteGenClient({
  baseUrl: BASE,
  getAuthToken: () => localStorage.getItem('litegen_api_key') ?? undefined,
  credentials: 'include',
  getCsrfToken,
  fetch: (input, init) => {
    const headers = new Headers(init?.headers);
    const org = client.getActiveOrgId();
    const app = client.getActiveAppId();
    if (org) headers.set('X-Litegen-Org-Id', org);
    if (app) headers.set('X-Litegen-App-Id', app);
    return fetch(input, { ...init, headers });
  },
  onError: onUnauthenticated,
});

// Initialize the active-tenant headers from localStorage so the very first
// requests (before TenantProvider resolves /auth/me) are already scoped.
{
  const initOrg = localStorage.getItem('litegen_active_org') ?? undefined;
  const initApp = localStorage.getItem('litegen_active_app') ?? undefined;
  if (initOrg || initApp) client.setActiveTenant(initOrg, initApp);
}
