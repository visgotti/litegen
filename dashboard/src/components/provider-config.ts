// The Gateway page's Configure form for one provider, kept free of React so it
// is unit-tested. It saves with PATCH /v1/admin/providers/{name}: a field sent
// is set, `null` clears it, and a field left out keeps its value, so what the
// API set and the form has no field for (headers, model mapping, options,
// region) survives a save.

/** What the form showed when it opened: the provider's view. */
export interface ProviderShown {
  /** The api_base in force, as GET serves it (`''` when none is set: the vendor default). */
  apiBase: string;
  enabled: boolean;
}

/** The form as typed. */
export interface ProviderForm {
  /** Blank keeps the stored key. */
  apiKey: string;
  /** Blank clears the runtime value: the env/YAML config's applies, else the vendor default. */
  apiBase: string;
  enabled: boolean;
}

/** The PATCH body: what the form changed, and nothing else. */
export function providerPatchBody(
  shown: ProviderShown,
  form: ProviderForm,
): { api_key?: string; api_base?: string | null; enabled?: boolean } {
  const body: { api_key?: string; api_base?: string | null; enabled?: boolean } = {};
  const key = form.apiKey.trim();
  if (key) body.api_key = key;
  const base = form.apiBase.trim();
  if (base !== shown.apiBase.trim()) body.api_base = base || null;
  if (form.enabled !== shown.enabled) body.enabled = form.enabled;
  return body;
}
