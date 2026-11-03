# MCP permissions

LiteGen's MCP server has no permission system of its own. A tool call runs
as the HTTP API request it wraps, as the caller, through
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
| `read` | The model catalog and generations, and `wait_for_generation` (it follows the job's stored record, which LiteGen refreshes every few seconds), plus request logs, usage stats, provider health, spend reports and logs, limits and end users |
| `admin` | Creating, editing, rotating, blocking and revoking keys; setting and resetting limits; managing end users; the audit log; clearing the cache; in single-tenant mode, the gateway's models, providers, routes and settings |

A key is bound to one organization and app, and never sees another
organization's data. Only owners and admins (or an `admin`-scope key) can
give a key the `admin` scope; LiteGen checks that when a key is created and
when its scopes are edited.

## OAuth

A client that signs in with OAuth (ChatGPT, Claude, Claude Code, VS Code,
Cursor and [others](README.md#connect-a-client)) acts as the person who
approved it. On LiteGen's sign-in page the person picks:

- **An organization.** The sign-in acts only there: `list_orgs` shows that
  one organization, and `create_org` is not offered.
- **An app** in it. Generations are recorded against that app, as for a key
  bound to it.
- **The scopes**, from those the client asked for:

| OAuth scope | Endpoint | Acts like the key scope |
|---|---|---|
| `ai:generate` | `/mcp` | `generate` |
| `ai:read` | `/mcp` | `read` |
| `admin:read` | `/admin/mcp` | `read` |
| `admin:write` | `/admin/mcp` | `admin` |

One sign-in is for one endpoint: a sign-in for `/mcp` is refused on
`/admin/mcp`, and the other way round. To use both, the client signs in to
each.

The person's role still applies, read again on every call: a sign-in may do
what both its scopes and the person's current role allow. Owners and admins
can approve every scope; members and viewers, every scope but `admin:write`.
A person moved to a smaller role loses what the new role does not allow at
once.

Signed-in users also reach the organization, app and member tools
(`list_orgs`, `invite_member`, `set_app_allowed_models`, …), which API keys
never can, because those routes act on a person's membership. Reading them
needs `admin:read`, changing them `admin:write`, and the role decides as it
does on the dashboard: an owner can transfer the org, a member cannot
invite.

**Ending a sign-in.** Its tokens stop working at once when:

- the person revokes it under **Connected apps** on the dashboard's Connect
  AI tools page, or with `revoke_connected_app` (`DELETE
  /v1/oauth/grants/{id}`); the organization's audit log records it;
- the client revokes its refresh token at LiteGen's revocation endpoint
  (RFC 7009);
- the person leaves the organization or their account is deactivated, or
  the app they picked is deleted.

People list and revoke only their own sign-ins; the master key can do it
for anyone through the HTTP API (`GET /v1/oauth/grants?user_id=…`). An
organization's owners and admins cannot revoke a member's sign-in, but
removing the member ends it.

Access tokens last an hour and refresh tokens 30 days; a deployment can
[change both](self-hosting.md#oauth). Each refresh replaces the refresh
token, and presenting a replaced one again ends the sign-in, in case it was
stolen.

**Keys outlive sign-ins.** An API key created through a sign-in (with
`admin:write`) is an ordinary key of the organization: revoking the sign-in
leaves it working. Revoke it on the **Keys** page.

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
