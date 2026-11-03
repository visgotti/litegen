# Claude Desktop, claude.ai and Claude mobile

Connect Claude's apps to LiteGen as a custom connector. A connector added
in one place works on the web, Desktop and mobile. Claude signs in with
OAuth: you approve it on LiteGen's sign-in page, and it acts as you, in the
organization and with the permissions you pick.

For Claude Code, see [Claude Code](claude-code.md): it takes an API key or
signs in the same way.

## Before you start

- A Claude plan with custom connectors: Free (one custom connector), Pro,
  Max, Ultra, Team or Enterprise. On Team and Enterprise, an Owner adds the
  connector for the organization.
- A LiteGen account.
- A LiteGen server Claude can reach over public HTTPS. The hosted service is
  `https://app.litegen.ai/api/mcp`. A self-hosted server needs a public
  `server.public_base_url`; see [self-hosting](../self-hosting.md#oauth).

LiteGen is added as a custom connector, not from Claude's connector
directory: the directory doesn't list connectors that generate images or
video. Custom connectors are not affected.

## Connect

**Free, Pro, Max or Ultra:**

1. In Claude, go to **Customize → Connectors** and click **Add custom
   connector**.
2. Name it `LiteGen` and enter the URL:

   ```text
   https://app.litegen.ai/api/mcp
   ```

   **Self-hosted:** `{public_base_url}/mcp`, for example
   `https://litegen.example.com/mcp`.
3. For authentication choose **Sign in now** (or **Sign in when needed**),
   and for the OAuth client **Use Claude's published identity**: LiteGen
   reads Claude's client details from the metadata document Anthropic
   hosts, so you set nothing up. **Register automatically** also works.
4. Add it. Claude opens LiteGen's sign-in page.

**Team or Enterprise:** an Owner goes to **Organization settings →
Connectors** (`https://claude.ai/admin-settings/connectors`), adds a custom
connector with the same URL and settings, and members then connect it from
**Customize → Connectors**. Each member signs in to LiteGen as themselves.

## Approve on LiteGen's sign-in page

The page shows that Claude asks to use LiteGen, that Claude is published by
`claude.ai`, and where you go back to. Pick:

- **Organization**: the one Claude acts in. It spends that organization's
  budget and sees only its data.
- **App it acts in**: where its generations are recorded.
- **Scopes**: what Claude may do. `ai:generate` generates images, video and
  3D models and spends budget; `ai:read` sees models and your generations.
  A scope your role in that organization can't grant is shown disabled.

Select **Approve**. Claude finishes connecting and lists LiteGen's tools.

## Use it

In a chat, ask: **"List LiteGen's image models."** Claude asks before each
tool call unless you allow it. Under **Customize → Connectors**, select
LiteGen to set each tool's permission, or to block the ones you don't
want; for example, block `generate_video` to keep a chat from spending on
video.

## Add the admin endpoint

Keys, budgets, spend, logs and gateway settings are on a second endpoint.
Add a second custom connector with:

```text
https://app.litegen.ai/api/admin/mcp
```

and approve `admin:read` (look) and, if you want Claude to change things,
`admin:write`. Only owners and admins can grant `admin:write`, the same
rule as for API keys with the `admin` scope.

## Limits

- **Time:** Claude allows about 240 seconds per tool call.
  `wait_for_generation` waits up to 45 seconds by default (`timeout_seconds`,
  at most 230) and then says to call it again, so long video and 3D jobs
  never hit the limit.
- **Size:** Claude takes about 150,000 characters per tool result. LiteGen
  returns images as URLs plus small previews, and trims long lists with a
  note; filter `list_models` by `media_type` rather than listing every
  model.

## With an API key instead (limited beta)

Organizations with Claude's **Request headers** beta can add the connector
with **No sign-in** and a header `authorization` set to `Bearer sk_live_…`.
Everyone who uses the connector then shares that one key and its
permissions, so prefer OAuth unless a shared service key is what you want.
On an OAuth connector, `Authorization` can't be set as a request header.

## Changing or removing it

A connector's name and URL can be edited under **Customize → Connectors**
(Owners: **Organization settings → Connectors**). Its authentication can't:
to change it, remove the connector and add it again.

To disconnect Claude on LiteGen's side, revoke it under **Connected apps**
on LiteGen's dashboard (**Connect AI tools**): its tokens stop working at
once.

## Self-hosted servers

- `server.public_base_url` must be the public HTTPS URL Claude and your
  browser reach; it is the OAuth issuer. See
  [self-hosting](../self-hosting.md#oauth).
- If a firewall limits who reaches the server, allow Anthropic's egress
  range `160.79.104.0/21`
  ([IP addresses](https://platform.claude.com/docs/en/api/ip-addresses)).
  Claude fetches the discovery documents from the same range.
- Claude Desktop can also run LiteGen through the `mcp-remote` bridge with
  an API key; see
  [clients that only start local servers](other-clients.md#clients-that-only-start-local-servers).

## Troubleshooting

- **"This app's sign-in link is invalid"** on LiteGen's page: LiteGen could
  not verify the client or its redirect. Remove the connector and add it
  again; for a self-hosted server, check it can reach `claude.ai` over HTTPS
  to read Claude's published identity.
- **"This sign-in request has expired"**: the request waited more than ten
  minutes, or was already decided. Connect again from Customize →
  Connectors.
- **Claude can't connect**: check the URL ends in `/mcp` and is reachable
  over public HTTPS; a self-hosted server also needs the root
  `/.well-known/oauth-*` routes ([self-hosting](../self-hosting.md#oauth)).
- **A tool is missing**: the scope you approved doesn't include it, or your
  role can't use it, or you blocked it in Customize → Connectors.
- More in [troubleshooting](../troubleshooting.md#oauth).

## Checked against

<https://claude.com/docs/connectors/custom/add-unlisted.md> and
<https://claude.com/docs/connectors/building/authentication.md> on
2026-11-03. Checked against docs: Claude's apps were not run. LiteGen's
OAuth was run end to end with Claude Code, which signs in with its own
published identity in the same way, and with the official MCP TypeScript
SDK.
