import { describe, expect, it } from 'vitest';
import { providerPatchBody } from './provider-config';

// The Gateway page's Configure form saves with PATCH /v1/admin/providers/{name}
// (fields sent are set, null clears, a field left out keeps its value), so a
// save changes what the form changed and nothing else: headers, model mapping,
// options and region set through the API survive it.

const shown = { apiBase: 'https://proxy.example/v1', enabled: true };

describe('providerPatchBody', () => {
  it('sends only what the form changed', () => {
    expect(providerPatchBody(shown, { apiKey: '', apiBase: 'https://other.example/v1', enabled: true }))
      .toEqual({ api_base: 'https://other.example/v1' });
    expect(providerPatchBody(shown, { apiKey: '', apiBase: 'https://proxy.example/v1', enabled: false }))
      .toEqual({ enabled: false });
  });

  it('sends a typed key, and none for a blank one: the stored key stays', () => {
    expect(providerPatchBody(shown, { apiKey: ' sk-new ', apiBase: 'https://proxy.example/v1', enabled: true }))
      .toEqual({ api_key: 'sk-new' });
    expect(providerPatchBody(shown, { apiKey: '   ', apiBase: 'https://proxy.example/v1', enabled: true })).toEqual({});
  });

  it("clears an emptied base URL, so the config's value applies, else the vendor default", () => {
    expect(providerPatchBody(shown, { apiKey: '', apiBase: '  ', enabled: true })).toEqual({ api_base: null });
  });

  it('sets a base URL where none was shown', () => {
    expect(providerPatchBody({ apiBase: '', enabled: true }, { apiKey: '', apiBase: ' https://proxy.example/v1 ', enabled: true }))
      .toEqual({ api_base: 'https://proxy.example/v1' });
  });

  it('sends a base URL shown with its user info redacted only once it is edited', () => {
    // GET serves `https://[redacted]@host/v1`; left as shown, the URL in force stays.
    const redacted = { apiBase: 'https://[redacted]@gw.example/v1', enabled: true };
    expect(providerPatchBody(redacted, { apiKey: '', apiBase: 'https://[redacted]@gw.example/v1', enabled: true })).toEqual({});
    expect(providerPatchBody(redacted, { apiKey: '', apiBase: 'https://gw.example/v2', enabled: true }))
      .toEqual({ api_base: 'https://gw.example/v2' });
  });
});
