LiteGen management: API keys, budgets and limits, end users, spend, request logs, audit log, organizations, apps, members and gateway configuration. Tools mirror the REST management API one to one and run with your key's scopes (read for reads, admin for changes) or your account role; tools you cannot use are not listed. Destructive tools (revoke, delete, block, reset, transfer) act immediately.

More:
- Look before you change: list_* and get_* first, and confirm ids with the user before anything destructive.
- Limits: set_*_limits takes max_budget_usd, soft_budget_usd, budget_duration (30s, 30m, 12h, 30d, 1w or 1mo), allowed_models, rpm_limit and max_parallel_requests. reset_*_limits zeroes the current window's spend.
- get_spend_report groups spend by day, model, provider, media_type, status, org, app, key, user, end_user or tag. Dates are RFC 3339 or YYYY-MM-DD.
- A new key's secret appears once, in the result of create_api_key or rotate_api_key. Hand it to the user; it cannot be read again.
- Key scopes: generate is AI-only access; read adds logs, spend, limits and end users; admin manages them. You can grant only scopes you hold.
- Gateway tools (models, providers, routes, settings) are for the platform admin, and in single-tenant mode for owners and admins.
