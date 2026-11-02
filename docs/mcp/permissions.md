# MCP permissions

LiteGen's MCP server has no permission system of its own. A tool call runs
as the HTTP API request it wraps, with the caller's own credentials, through
the same authentication, scope checks, tenant checks, budgets, rate limits,
idempotency and audit log as a direct API call. MCP only decides which tools
to list: a caller sees exactly the tools its credential could call. Calling
a tool you cannot see answers `Unknown tool`.

## Keys: AI, management, or both

An API key holds one or more of three scopes. The dashboard's **Keys** page
offers them as presets, the way LiteLLM offers key types:

| Preset | Scopes | LiteLLM key type | Use it for |
|---|---|---|---|
| AI | `generate` | `llm_api` | An app or agent that generates. Sees the 21 AI tools on `/mcp`, and nothing of the management API. |
| Management | `read,admin` | `management` | An agent that manages keys, budgets, end users and settings on `/admin/mcp`. Cannot generate. |
| AI + management | `generate,read,admin` | `default` | Both endpoints, everything the org allows a key. |
| Read-only | `read` | `read_only` | Monitoring and reporting: models, generations, request logs, spend and limits, on both endpoints. Changes nothing. |
| Custom | any mix | — | Any other combination. |

What each scope opens:

| Scope | Tools it opens |
|---|---|
| `generate` | Generating images, video and 3D, cost quotes, job status, `wait_for_generation`, mesh conversion, the model catalog, and the org's generations and their files |
| `read` | The model catalog and generations, plus request logs, usage stats, provider health, spend reports and logs, limits and end users |
| `admin` | Creating, editing, rotating, blocking and revoking keys; setting and resetting limits; managing end users; the audit log; clearing the cache; in single-tenant mode, the gateway's models, providers, routes and settings |

A key is bound to one organization and app, and never sees another
organization's data. Only owners and admins (or an `admin`-scope key) can
give a key the `admin` scope; LiteGen checks that when a key is created and
when its scopes are edited.

## Signed-in users

When OAuth support ships, a client that signs in (ChatGPT, claude.ai,
Claude Desktop, and any client that prefers it) acts as the person who
approved it, with their role in the organization they picked, narrowed to
the scopes they approved:

| OAuth scope | Endpoint | Acts like |
|---|---|---|
| `ai:generate` | `/mcp` | the `generate` key scope |
| `ai:read` | `/mcp` | the `read` key scope |
| `admin:read` | `/admin/mcp` | the `read` key scope |
| `admin:write` | `/admin/mcp` | the `admin` key scope |

A signed-in user can also use the organization, app and member tools
(`list_orgs`, `invite_member`, `set_app_allowed_models`, …), which API keys
never can, because those routes act on a person's membership. Their role
decides as it does on the dashboard: an owner can transfer the org, a member
cannot invite.

## The master key

The deployment's master key is the platform admin.

- In single-tenant mode it acts inside the default organization and sees
  everything a key can, plus the gateway tools.
- In hosted mode it belongs to no organization. On `/mcp` it sees only the
  model catalog, because every other AI tool acts inside an organization;
  on `/admin/mcp` it sees the gateway, user and organization tools. Use an
  organization's key, or sign in, to generate.

## Which tool needs what

Each tool's requirement is listed in the [tool reference](tools.md). The
kinds:

| Requirement | Who passes |
|---|---|
| a scope (`generate`, `read`, `admin`) | a credential holding it |
| `generate` or `read` | either |
| org read access | the platform admin; a key with `read` or `admin` in that org; a member whose role may read it |
| org write access | the platform admin; a key with `admin` in that org; a member whose role may change it |
| a member permission | the platform admin, or a signed-in member whose role has it (never an API key) |
| platform admin | the master key (in single-tenant mode, owners and admins) |
| a signed-in user | OAuth sign-in only |

## Worked examples

**A generation pipeline.** Create an **AI** key and connect it to `/mcp`.
The agent can list and read models, quote, generate, wait for jobs and
fetch files. It cannot read the org's spend or logs, or change anything on
`/admin/mcp`. Give the key a budget (`set_key_limits`, or the Budgets page)
to cap what it spends.

**An observability agent.** Create a **Read-only** key and connect it to
`/admin/mcp`. The agent can read request logs, usage stats, spend reports
and limits, and answer "what did we spend on video this week?", but it
cannot generate or change a setting.

**A key-management assistant.** Create a **Management** key and connect it
to `/admin/mcp?toolsets=keys,budgets`. The agent sees only the key and
budget tools: create and rotate keys, set limits, block a leaking key.
Destructive tools (revoke, delete, block, reset, transfer) are marked as such,
so clients ask before running them.

## Creating keys

- **Dashboard:** Keys → pick a preset → Create Key. The secret appears once.
- **API or MCP:** `create_api_key` with `{"name": "…", "scopes": "generate"}`
  (needs an `admin`-scope credential). The secret is in the result, once.
