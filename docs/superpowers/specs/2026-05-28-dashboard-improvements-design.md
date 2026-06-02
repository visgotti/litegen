---
title: Dashboard improvements — Playground, observability, granular CRUD
status: approved
owner: joe
date: 2026-05-28
---

# Goal

Take the litegen dashboard from "shows pre-existing data and can CRUD API keys" to a tool that can drive real generations, observe traffic in real time, and manage every server-side entity end-to-end. Extend the single Playwright god test to click every new CRUD path with zero mocks.

# Out of scope

- Per-model schema editing (yaml on disk stays the source of truth).
- Cancelling in-flight provider generations (we only update local state).
- Real-provider Playground (mock provider only this round; toggle for real providers is a follow-up).
- CSV export, multi-tenant org switching, dark/light theme toggle.

# Backend additions

Five small endpoints. None require migrations beyond what's already on the branch.

| Endpoint | Auth scope | Behaviour |
|---|---|---|
| `GET /v1/generations` | `read` | Paginated list of rows from the `generations` table. Returns rows whose `key_id` matches the requester's key (or all rows if master-key). Query: `?page=N&per_page=25`. |
| `PATCH /v1/generations/{id}` | `read` | Body `{ "status": "cancelled" }`. Soft-cancels: sets `status = 'cancelled'`, `completed_at = NOW()`, leaves `result_url` untouched. 403 if the key doesn't own it. Only `pending` and `processing` rows can be cancelled. |
| `POST /v1/keys/{id}/rotate` | `admin` | Atomic: revokes the old key, creates a new one with the same `name`, `scopes`, `token_quota`, `rpm_limit`, `webhook_url`, `expires_at`. Returns the new key value once. |
| `POST /v1/keys/{id}/test-webhook` | `admin` | 400 if the key has no `webhook_url`. Otherwise dispatches one synthetic terminal-state payload (id `webhook-test-<uuid>`, `status = completed`, `provider = test`) via the existing `dispatch_webhook` and returns `{ "status_code": N, "delivered": bool, "error": optional }` with the receiver's response. |
| `GET /v1/logs?model=…&provider=…&status=…&from=…&to=…` | `read` | Extend existing list endpoint with these optional filters. Date strings are ISO 8601. |

# Frontend additions

Five new or expanded routes plus shared infrastructure.

## Shared

### `apiFetch` helper
- All API calls go through one fetch wrapper. Reads `localStorage.litegen_api_key`, sends as `Authorization: Bearer …`.
- On 401, clears localStorage and triggers AuthBar re-show.
- On 402/403/429, surfaces a toast (top-right, 4s) instead of a global alert.

### `useAutoRefresh(callback, intervalMs, enabled)` hook
- Sets up a setInterval that fires the callback. Cleans up on unmount. Re-bound when interval changes.

### Toast container
- Mounted at the App root. Pure CSS, no extra dep. Three levels: info / warning / error.

## Playground page (`/playground`)

Layout: 2 columns on desktop, stacked on mobile.

**Left column — Input form:**
- Model picker (dropdown). Populated from `GET /v1/models` filtered to `media_type = image` AND `provider = mock`. (We restrict to mock per design decision.)
- Prompt textarea (multiline, required).
- Negative prompt textarea (optional).
- Size select: from the picked model's `params.size` enum.
- Seed input (number).
- N input (number, 1-4).
- Strict toggle (default on).
- "Generate" button (disabled while in flight).

**Right column — Output panel:**
- Tabs: `Image`, `Request JSON`, `Response JSON`.
- Image tab: renders `data[0].b64_json` (decode + `<img>`) or `data[0].url` (`<img src>`).
- Request JSON: pretty-printed body that was POSTed.
- Response JSON: pretty-printed response.

**Bottom — History strip:**
- Reads from `localStorage.litegen_playground_history` (cap 10).
- Each entry shows truncated prompt + model + timestamp.
- Click → repopulates the input form (does NOT re-fire automatically).
- Trash icon → removes from history.
- "Re-submit" button on each row → repopulates and fires.

`data-testid`s: `playground-model`, `playground-prompt`, `playground-generate`, `playground-image`, `playground-history-row-{i}`, `playground-history-delete-{i}`, `playground-history-resubmit-{i}`, `playground-tab-{image|request|response}`.

## Generations page (`/generations`)

- Table columns: id (mono, click to expand), model, status badge, cost, created, actions.
- Polls `GET /v1/generations` every 3s when any row is in `pending` or `processing`.
- Click row → inline expand showing full JSON + (if completed) embedded `<video controls>` of `result_url`.
- Action: `Cancel` (only on pending/processing rows). Calls `PATCH .../{id}` with `{status: "cancelled"}`. Disappears from the active set on success.
- Pagination via `?page=N` controls at the bottom.

`data-testid`s: `gen-row-{id}`, `gen-status-{id}`, `gen-cancel-{id}`, `gen-expand-{id}`, `gen-detail-{id}`.

## Overview upgrades

Existing stat cards + provider/model pie charts stay. Add:

- **Top-right Auto-refresh toggle:** label "Auto-refresh: 5s", checkbox. When on, re-fetches stats + logs (for cost chart) + keys (for quota table) every 5s. State stored in `localStorage.litegen_overview_autorefresh`.
- **Cost over time chart (new):** Bar chart, 24 hourly buckets. Computed client-side from `GET /v1/logs?per_page=500` (oldest first). x-axis: hour-of-day, y-axis: cost. Renders as `<svg>` via recharts `BarChart`.
- **Quota usage table (new):** below cost chart. For each non-revoked key with `token_quota != null`: name, prefix, usage bar (`tokens_used / token_quota`), remaining $, RPM.

`data-testid`s: `overview-autorefresh`, `overview-cost-chart`, `overview-quota-table`, `overview-quota-row-{id}`.

## Logs upgrades

- **Filter row:** model dropdown (populated from /v1/models), provider dropdown, status dropdown (pending/completed/failed), from/to date inputs (`<input type="date">`). "Apply" button. "Clear" button.
- Filter state syncs to URL query params for shareability.
- Click row → inline expand showing full log JSON.
- Pagination unchanged.

`data-testid`s: `logs-filter-{model|provider|status|from|to|apply|clear}`, `logs-row-{id}`, `logs-detail-{id}`.

## Keys granular CRUD

Existing create/edit/revoke stays. Add 3 new actions per row:

1. **Copy prefix** — clipboard, shows "Copied!" toast.
2. **Rotate** — calls `POST /v1/keys/{id}/rotate`. Shows the new secret in a one-time banner like the original create flow. Old row marked revoked. Reloads list.
3. **Test webhook** — visible only if the row has `webhook_url`. Calls the test-webhook endpoint and shows result inline next to the button: "200 OK" / "503 Service Unavailable" / "Network error: …".

`data-testid`s: `key-copy-prefix-{id}`, `key-rotate-{id}`, `key-test-webhook-{id}`, `key-rotate-result-banner`, `key-test-webhook-result-{id}`.

## Models filters

Top of page: provider dropdown (auto-populated from loaded models), media_type radio group (image / video / all), capability checkboxes (`text_to_image`, `image_to_image`, `text_to_video`, etc — derived from union of all model capability keys).

Detail panel adds a **Copy as curl** button. Generates:
```
curl -X POST $LITEGEN_BASE/v1/images/generations \
  -H "Authorization: Bearer $LITEGEN_KEY" \
  -H "Content-Type: application/json" \
  -d '{"model":"<id>","prompt":"a photo of a cat","n":1}'
```

`data-testid`s: `models-filter-{provider|media_type|capability}`, `models-copy-curl-{id}`.

# God test extension

The single Playwright `test(...)` in `dashboard/e2e/god-test.spec.ts` grows. New steps inserted between the existing Health and Keys steps:

1. **Playground:** navigate, pick `mock/image-gen`, fill prompt "a playwright cat", click Generate. Assert `playground-image` `<img>` is visible AND has a non-empty `src`. Switch to Request JSON tab, assert body has `"model": "mock/image-gen"` and `"prompt": "a playwright cat"`. Switch to Response JSON, assert `id` field exists. Assert one history row appeared. Click it → form repopulates. Click Re-submit → second history row.

2. **Generations:** in Playground, switch the model picker filter to video, generate a video. Navigate to `/generations`. Wait for the row to appear. Click to expand → assert detail JSON shows. Click Cancel → assert status flips to `cancelled`. Reload page → assert row still shows cancelled.

3. **Overview auto-refresh:** navigate, toggle Auto-refresh on, wait 6s, assert "Total Requests" counter is non-zero (real traffic from the playground steps). Toggle off.

4. **Overview cost chart + quota table:** assert `overview-cost-chart` is present. Assert `overview-quota-table` has at least one row (the one we created earlier with quota = 10).

5. **Logs filters:** apply filter `model = mock/image-gen`, click Apply. Assert URL has `?model=mock%2Fimage-gen`. Click a row → assert detail panel opens. Click Clear → assert URL has no filter.

6. **Keys rotate + test-webhook:** rotate the existing playwright key → assert new secret banner appears. Set a webhook URL on a new key pointing at a tiny Express handler started by the test on port 5180 → click Test webhook → assert receiver got one POST AND the inline result shows "200 OK".

7. **Models filter + copy curl:** filter to `mock`, click row, click Copy as curl. Read clipboard, assert it includes the model id.

Everything still in ONE `test(...)` block. Video on. No mocks of the litegen backend.

# Acceptance

- `cd litegen-core && cargo test --lib` — all green, no decrease in pass count from baseline (107).
- `cd litegen-core && cargo clippy --lib --no-deps -- -D warnings` — clean.
- `cd litegen-core && cargo build --release` — clean.
- `cd dashboard && npm run build` — clean.
- `cd dashboard && PLAYWRIGHT_MASTER_KEY=… npx playwright test` — 1 pass, video.webm produced.
- Binary still starts and shuts down cleanly on SIGINT.

# Implementation order

1. Backend endpoints (5 new) + unit tests.
2. Shared frontend infrastructure (`apiFetch`, `useAutoRefresh`, toast container).
3. Playground page + History.
4. Generations page + polling.
5. Overview upgrades (cost chart, quota table, auto-refresh).
6. Logs upgrades (filters, detail expand).
7. Keys granular actions.
8. Models filters + copy-curl.
9. Extend god test step-by-step, running after each addition.
10. Final clippy + build + Playwright run.
