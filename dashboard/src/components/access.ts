// What litegen does with stored outputs (docs/storage-access.md): whether it
// keeps them, for how long, and who may open them. The form behind the app's
// access panel and the gateway's settings, kept free of React so it is
// unit-tested. Its ranges are the server's: the gateway's settings serve them
// (`GatewaySettingsView.limits`); where none are served, a whole number goes
// as typed and the server's 400 says its range.
import type { AccessSettings, PutAppAccessRequest, SettingRange } from '@litegen/sdk';

/** The served ranges the form holds to, when the page has them. */
export interface AccessLimits {
  ttl?: SettingRange;
  retentionDays?: SettingRange;
}

const outside = (n: number, range: SettingRange | undefined) => !Number.isInteger(n) || (range !== undefined && (n < range.min || n > range.max));
const within = (range: SettingRange | undefined) => (range ? ` from ${range.min} to ${range.max}` : '');

/** The form as typed: `''` in any field means "use the next layer". */
export interface AccessForm {
  mode: string;
  ownerScope: string;
  ttl: string;
  /** `'true'`, `'false'` or `''`. */
  keepOutputs: string;
  retentionDays: string;
  signedLinks: string;
}

/** The request body for the form, or why it can't be sent. */
export function accessBody(form: AccessForm, limits: AccessLimits = {}): { body?: PutAppAccessRequest; error?: string } {
  const body: PutAppAccessRequest = {};
  if (form.mode) body.mode = form.mode as PutAppAccessRequest['mode'];
  if (form.ownerScope) body.owner_scope = form.ownerScope as PutAppAccessRequest['owner_scope'];
  const ttl = form.ttl.trim();
  if (ttl !== '') {
    const n = Number(ttl);
    if (outside(n, limits.ttl)) return { error: `Link life must be a whole number of seconds${within(limits.ttl)}.` };
    body.signed_url_ttl_seconds = n;
  }
  if (form.keepOutputs === 'true' || form.keepOutputs === 'false') body.keep_outputs = form.keepOutputs === 'true';
  const days = form.retentionDays.trim();
  if (days !== '') {
    const n = Number(days);
    if (outside(n, limits.retentionDays)) {
      return { error: `Retention must be a whole number of days${within(limits.retentionDays)}; 0 keeps files until they are deleted.` };
    }
    body.retention_days = n;
  }
  if (form.signedLinks) body.signed_links = form.signedLinks as PutAppAccessRequest['signed_links'];
  return { body };
}

function duration(seconds: number): string {
  if (seconds % 86_400 === 0) return `${seconds / 86_400} day${seconds === 86_400 ? '' : 's'}`;
  if (seconds % 3600 === 0) return `${seconds / 3600} h`;
  if (seconds % 60 === 0) return `${seconds / 60} min`;
  return `${seconds} s`;
}

const WHO: Record<AccessSettings['owner_scope'], string> = {
  key: 'the key that made the file',
  app: 'any key of the app',
  org: 'any key of the org',
};

/** One line saying whether files are kept, how links work, who may open
 * them, and when kept files are deleted. */
export function describeAccess(s: AccessSettings): string {
  const parts: string[] = [];
  if (!s.keep_outputs) parts.push('keeps nothing');
  switch (s.mode) {
    case 'public':
      parts.push(`public links · file route open to ${WHO[s.owner_scope]}`);
      break;
    case 'signed':
      parts.push(`signed links${s.signed_links === 'litegen' ? ' by litegen' : ''} for ${duration(s.signed_url_ttl_seconds)} · open to ${WHO[s.owner_scope]}`);
      break;
    case 'private':
      parts.push(`private: the file route only · open to ${WHO[s.owner_scope]}`);
      break;
    default:
      parts.push(String(s.mode));
  }
  if (s.retention_days > 0) parts.push(`deleted after ${s.retention_days} day${s.retention_days === 1 ? '' : 's'}`);
  return parts.join(' · ');
}
