# Providers Page Design

**Date:** 2026-06-20
**Status:** Approved

## Problem

Provider credential management is buried inside Organization.tsx alongside unrelated concerns (org rename, app CRUD, BYO S3). There is no concept of scoping which models are available to an org or to individual apps — any key under any app can call any model the app has credentials for, with no allowlist layer.

## Goal

1. Extract provider credential management into a dedicated `/providers` page.
2. Add an org-level model pool: owners/admins pick which models from the global catalog the org makes available.
3. Add per-app model access: each app either inherits the full org pool ("all") or selects a subset.
4. Apps cannot add their own models — they can only draw from what the org has enabled.

## Tenancy hierarchy (unchanged)

```
Org
└── App (multiple per org)
    ├── Provider Credentials  ← stays per-app for now
    ├── BYO S3 Storage
    ├── ApiKeys
    └── App model access      ← new (subset of org pool)

Org allowed-model pool        ← new (owner/admin configures)
```

## Backend changes

### New DB tables

```sql
-- Which models an org has enabled (subset of global catalog)
CREATE TABLE org_allowed_models (
    org_id   TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (org_id, model_id)
);

-- Per-app model access: mode row (one per app) + model rows (only when mode='select')
CREATE TABLE app_model_access (
    app_id TEXT NOT NULL PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    mode   TEXT NOT NULL DEFAULT 'all'  -- 'all' | 'select'
);

CREATE TABLE app_model_access_ids (
    app_id   TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    PRIMARY KEY (app_id, model_id)
);
```

### New API routes

```
GET  /v1/orgs/{orgId}/allowed-models
→ { models: string[] }
     List of model IDs the org has enabled. Empty = none enabled.

PUT  /v1/orgs/{orgId}/allowed-models
body: { models: string[] }
     Replaces the org's allowed model list. Requires org:write (owner/admin).

GET  /v1/apps/{appId}/allowed-models
→ { mode: "all" | "select", models: string[] }
     "all"    = app gets everything in the org pool.
     "select" = app gets only the listed subset (must be ⊆ org pool; backend enforces).

PUT  /v1/apps/{appId}/allowed-models
body: { mode: "all" | "select", models: string[] }
     Replaces the app's model access config. Requires app:write (owner/admin).
     Backend validates that all model_ids in "select" mode are in the org pool.
```

Permission gate: both PUT routes require `org:write` / `app:write` — owner or admin only.

## Frontend: Providers.tsx

New page at `/providers`. Three cards, top-to-bottom. All three visible/editable only for owner/admin (`activeOrgRole === 'owner' || activeOrgRole === 'admin'`). Viewer/member sees a 403-style message.

### Card 1 — Org model pool

- Fetches `GET /v1/models` (global catalog) + `GET /v1/orgs/{orgId}/allowed-models` (current selection).
- Renders models grouped by provider. Each row: model name, type badge (image/video), checkbox.
- Save → `PUT /v1/orgs/{orgId}/allowed-models` with the checked model IDs.
- If nothing selected: warning badge "No models enabled — apps have no access".

### Card 2 — App credentials

- Exact lift from `Organization.tsx` provider credentials section (lines 312–443).
- No logic changes. Uses existing `activeApp` context and `client.apps.providerCredentials.*` SDK calls.
- App switcher in header determines which app's credentials are shown.

### Card 3 — App model access

- Fetches `GET /v1/apps/{appId}/allowed-models` on `activeApp` change.
- Toggle: `[● All org models] [ Select]`
  - "All": saves `{ mode: "all", models: [] }` immediately on toggle.
  - "Select": shows checkboxes for each model in the org pool. Unchecked = disabled for this app.
- Save → `PUT /v1/apps/{appId}/allowed-models`.

## Navigation changes

- Add "Providers" nav entry (lucide `Plug` icon) in `App.tsx` between Health and API Keys.
- Route: `<Route path="/providers" element={<Providers />} />`
- Remove the provider credentials card from `Organization.tsx` (lines 312–443).
- Organization.tsx retains: org name, apps CRUD, BYO S3 storage.

## Permission gating pattern

Consistent with existing pages — no `RequirePermission` route wrapper. The page renders a message for insufficient role:

```tsx
if (activeOrgRole !== 'owner' && activeOrgRole !== 'admin') {
  return <div>You need admin or owner access to manage providers.</div>;
}
```

## Out of scope

- Moving provider credentials to org level (deferred).
- Per-key model allowlists (deferred).
- Cost caps or display name overrides per model in the org pool (deferred).
