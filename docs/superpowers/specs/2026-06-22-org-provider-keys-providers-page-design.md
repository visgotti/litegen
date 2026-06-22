# Org-Scoped Provider Keys + Provider-Centric Providers Page

**Date:** 2026-06-22
**Status:** Draft (pending review)

## Problem

Provider credentials are **app-scoped** today (`/v1/apps/{app_id}/provider-credentials`), so keys are re-entered per app and the request path resolves them by `app_id`. The Providers page is three stacked cards (org model pool, app credentials, app model access) that don't match the litellm mental model of "configure a provider once, then see/enable its models."

Two concrete weaknesses found while verifying the current page:

- The org model pool is a flat ~100-checkbox list grouped only loosely by provider prefix, with no per-model capability/feature view. Clicking a model anywhere (the standalone `/models` page) only dumps raw schema JSON.
- **Card 1↔Card 3 coupling bug:** the app-access "select" list renders from the *live, unsaved* org-pool state. Enabling a model in the pool without saving, then selecting it for an app, is rejected by the backend (`400 model_not_in_org_pool`) with a message that reads as a contradiction, and the rejected checkbox stays out of sync with the server.

## Goals

1. Provider API keys are configured **once at the org level**; every app in the org transparently reuses the same key.
2. The per-app layer is purely a **granular allowlist** (which models an app may call), reusing the org key — no per-app keys.
3. The Providers page becomes **provider-centric**: one collapsible section per provider containing its key config, its models, and a configured/needs-key status.
4. Clicking a model opens a readable **feature detail**: capabilities, params + allowed values, pricing, and a copy-as-curl example.
5. Enabling a model **auto-saves**, which structurally removes the coupling bug.

## Non-goals

- Per-app key overrides (org key is the only key).
- New per-model metadata — cost caps / display-name overrides remain deferred.
- Changing app-model-access semantics (`all` | `select` subset of the org pool).

## Tenancy after this change

```
Org
├── Provider Credentials   ← MOVED to org scope (was per-app)
├── Allowed-model pool      ← org scope (unchanged)
└── App (many per org)
    └── App model access    ← per-app granular allowlist (unchanged); reuses the org key
```

## Backend

### DB

- New table `org_provider_credentials`, mirroring the existing per-app `provider_credentials` schema but keyed by `(org_id, provider)`:

  ```sql
  CREATE TABLE org_provider_credentials (
    org_id       TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    provider     TEXT NOT NULL,
    ciphertext   TEXT NOT NULL,
    nonce        TEXT NOT NULL,
    display_hint TEXT,
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (org_id, provider)
  );
  ```

  Migration `20240101000013_org_provider_credentials.sql` in **both** `migrations/sqlite/` and `migrations/postgres/` (matches the existing dual-file convention, e.g. `…012_model_allowlists.sql`).

- **Data migration (best-effort copy-up):** for each `(org, provider)`, copy the credential from the first app in that org that has one into `org_provider_credentials` (first-writer-wins per provider). Then `DROP TABLE provider_credentials` (the per-app table). No re-entry required; live keys preserved.

- **DB trait** (`db/trait_def.rs` + `db/sqlite.rs` + `db/postgres.rs`): replace the four app-keyed methods with org-keyed equivalents:
  `upsert_org_provider_credential(org_id, provider, ciphertext, nonce, display_hint)`,
  `get_org_provider_credential(org_id, provider) -> Option<(ciphertext, nonce)>`,
  `list_org_provider_credentials(org_id) -> Vec<ProviderCredentialInfo>`,
  `delete_org_provider_credential(org_id, provider) -> bool`.

### API routes

In `api/handlers/mod.rs` router + `api/handlers/orgs.rs`:

- **Add** (handlers are a near-direct port of the existing app ones at `orgs.rs:834–1000`, swapping the `org_for_app(app_id)` resolution for a direct `org_id` path param):
  - `GET  /v1/orgs/{org_id}/provider-credentials` — list. Requires org membership (read).
  - `POST /v1/orgs/{org_id}/provider-credentials` — upsert. Requires `Permission::ProviderCredWrite` (owner/admin). Encrypts `body.credentials` (the pooled-key blob) with `secrets_key`, derives `display_hint`, calls `upsert_org_provider_credential`.
  - `DELETE /v1/orgs/{org_id}/provider-credentials/{provider}` — delete. Requires `ProviderCredWrite`.
- **Remove** the three `/v1/apps/{app_id}/provider-credentials[...]` routes + handlers.

### Request-time credential resolution

The two spots that resolve a key by `app_id` switch to org resolution:

- `api/handlers/mod.rs:2504` — `get_provider_credential(app_id, provider)` → resolve org via the existing `org_for_app` helper (`orgs.rs:109`, returns `(Application, org_id)`), then `get_org_provider_credential(org_id, provider)`.
- `proxy/poller.rs:188` — same swap; the async polling path has the app id on the generation record, so org is derivable. Add a small cached app→org lookup if this proves hot.

### SDK (`sdks/typescript`)

- Add `client.orgs.providerCredentials` namespace (`list` / `create` / `delete`) mirroring the current app namespace; remove `client.apps.providerCredentials` and `AppProviderCredentialsNamespace`.
- Regenerate OpenAPI-derived types for the new org routes.

## Frontend (`dashboard/src/pages/Providers.tsx` + new components)

### Page structure

- On load, fetch: `/v1/providers` (per-provider `fields`, `pool_field`, `modalities`), `/v1/models` (capabilities/pricing/tags), `/v1/orgs/{id}/provider-credentials` (which providers are configured), `/v1/orgs/{id}/allowed-models` (the org pool).
- Render a **`ProviderSection`** per provider, sorted configured-first then alphabetical; configured sections expanded, others collapsed. Each section:
  - **Header:** provider name · status dot (● configured / ○ needs key — derived from the org-credentials list, since `is_available` is currently a hardcoded stub; `mock` always ●) · "N enabled" count · expand chevron.
  - **Key block (org-scoped):** reuse the existing credential form — per-provider `fields`, multi-row keys with load-balance weights, masked `display_hint` + Delete — pointed at `client.orgs.providerCredentials`. The provider is fixed by the section (no provider dropdown).
  - **Models block (org-scoped):** the provider's models; each row = enable checkbox (org pool) + name + type badge + price + click→detail drawer.
- **Model enable = auto-save toggle:** toggling immediately `PUT`s the org pool with the new set and shows a toast. No "Save pool" button, no dirty state.
- **App model access** card retained below, unchanged (per-app `all` | `select` subset) — the granular app layer. It reads the now-always-saved org pool.

### Coupling-bug fix

Because the pool auto-saves, the app-access "select" list always reflects the persisted pool — the unsaved-divergence path is gone. Belt-and-suspenders: refetch the pool when the app-access card mounts/expands.

### `ModelDetail` shared component (right-side drawer)

Same drawer pattern as today's `/models` panel. Given a model id, fetch `client.models.getSchema(id)` and render:

- **Capabilities:** only the truthy ones, as labeled rows/chips — text→image, image→image, inpainting, text→video, image→video, first/last-frame, max reference images, supported sizes, max duration.
- **Params & allowed values:** iterate schema `params`; per param show name, kind, allowed values/range, default. Reuse the playground spec readers in `playground/params.ts` (`sizeEnumOptions`, `defaultForSpec`, etc.) where possible.
- **Pricing:** `base_cost_usd` + `variable_pricing`.
- **Example request:** copy-as-curl (lift `buildCurl` from `Models.tsx`).

Adopt this component on `/models` too, replacing its raw-JSON panel. `/models` stays as the global reference table.

## Permission gating

Unchanged pattern: page editable by owner/admin only. Org credential writes require `ProviderCredWrite`; pool writes require `OrgWrite`.

## Testing

- **Backend:** unit/integration for org cred upsert/get/list/delete; request-time resolution returns the org key (include a Postgres regression per the strict-decode gotcha); migration copy-up picks first-writer-wins and drops the app table.
- **E2E (multitenant):** the current `e2e-mt/multitenant.spec.ts` credential steps drive the **removed** app-cred UI and must be rewritten to the org per-provider section. Add coverage: configure an org key → model enable auto-saves to the pool → model-detail drawer shows capabilities/params/pricing/curl → a second app in the org reuses the org key for a real generation.

## Rollout / phases

- **Phase 1 — backend:** DB table + trait + migration (copy-up) + org routes + request-time resolution switch + SDK. Verify keys resolve at generation time for any app in the org.
- **Phase 2 — frontend:** per-provider sections + auto-save toggles + `ModelDetail` drawer + `/models` adoption + e2e updates.

Phase 2 depends on Phase 1 (the page consumes the org-credential endpoints).
