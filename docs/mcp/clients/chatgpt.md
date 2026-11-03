# ChatGPT

Connect ChatGPT to LiteGen as a custom MCP server. ChatGPT signs in with
OAuth: you approve it on LiteGen's sign-in page, and it acts as you, in the
organization and with the permissions you pick.

## Before you start

- A ChatGPT account whose plan and workspace let you add custom MCP
  servers. In a workspace, an admin controls this with the **Create custom
  MCP servers** permission.
- A LiteGen account: ChatGPT cannot send an API key or any custom header,
  so OAuth is the only way in.
- A LiteGen server ChatGPT can reach over public HTTPS. The hosted service
  is `https://app.litegen.ai/api/mcp`. A self-hosted server needs a public
  `server.public_base_url`; see [self-hosting](../self-hosting.md#oauth).

## Connect

1. Go to [chatgpt.com/plugins](https://chatgpt.com/plugins), select the plus
   button, then **Add custom MCP server**.
2. Enter a name (`LiteGen`) and a description, for example "Generate images,
   video and 3D models".
3. Under **Connection**, enter the server URL:

   ```text
   https://app.litegen.ai/api/mcp
   ```

   **Self-hosted:** `{public_base_url}/mcp`, for example
   `https://litegen.example.com/mcp`.
4. For authentication, choose **OAuth**. ChatGPT finds LiteGen's sign-in
   from the server itself; leave the client fields empty.
5. Review the risk warning, select **I understand and want to continue**,
   then **Create as a plugin**, and install it.
6. The first time ChatGPT connects (to list LiteGen's tools, or when you
   install the plugin), it opens LiteGen's sign-in page. Sign in to LiteGen
   if asked, then approve below. ChatGPT then lists the tools it found.

## Approve on LiteGen's sign-in page

The page shows that ChatGPT asks to use LiteGen and where you go back to
(`chatgpt.com`). Pick:

- **Organization**: the one ChatGPT acts in. It spends that organization's
  budget and sees only its data.
- **App it acts in**: where its generations are recorded.
- **Scopes**: what ChatGPT may do. `ai:generate` generates images, video and
  3D models and spends budget; `ai:read` sees models and your generations.
  A scope your role in that organization can't grant is shown disabled.

Select **Approve**, and ChatGPT finishes connecting. **Deny** sends it away
with nothing.

## Use it

Start a new conversation, type `@` in the prompt box and select LiteGen,
then ask, for example: **"List LiteGen's image models."** ChatGPT treats any
tool not marked read-only as a write and asks you to confirm it in the
conversation; generating spends budget, so it asks before
`generate_image`, `generate_video` and `generate_3d_model`.

## Add the admin endpoint

Keys, budgets, spend, logs and gateway settings are on a second endpoint.
Add a second custom MCP server with:

```text
https://app.litegen.ai/api/admin/mcp
```

and approve `admin:read` (look) and, if you want ChatGPT to change things,
`admin:write`. Only owners and admins can grant `admin:write`, the same
rule as for API keys with the `admin` scope. One sign-in is for one
endpoint, so the two servers are approved separately.

## Long jobs

Videos and 3D models take minutes. `generate_video` and `generate_3d_model`
return a job id at once, and `wait_for_generation` waits up to 45 seconds
by default (`timeout_seconds`, at most 230) and then says to call it again,
so no call runs into ChatGPT's limits.

## After you change something

When LiteGen's tools change (a new version, or a different
[toolset](../README.md#toolsets) in the URL), open the plugin at
[chatgpt.com/plugins](https://chatgpt.com/plugins) and select **Refresh**.

To disconnect ChatGPT, revoke it under **Connected apps** on LiteGen's
dashboard (**Connect AI tools**): its tokens stop working at once.

## Self-hosted servers

- `server.public_base_url` must be the public HTTPS URL ChatGPT and your
  browser reach; it is the OAuth issuer. See
  [self-hosting](../self-hosting.md#oauth).
- If a firewall limits who reaches the server, allow OpenAI's
  [egress IP ranges](https://developers.openai.com/api/docs/guides/ip-addresses).
- A server that must stay off the internet can be reached through OpenAI's
  [Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)
  (the **Tunnel** connection in step 3). The sign-in still runs in your
  browser, which must reach the server's `public_base_url`.

## Troubleshooting

- **"This app's sign-in link is invalid"** on LiteGen's page: the request
  named a client or redirect LiteGen doesn't know. Remove the plugin and
  add it again.
- **"This sign-in request has expired"**: the request waited more than ten
  minutes, or was already approved or denied. Start the connection again
  in ChatGPT.
- **ChatGPT can't connect**: check the server URL ends in `/mcp` and is
  reachable over public HTTPS; try it with
  [MCP Inspector](mcp-inspector.md). A self-hosted server also needs the
  root `/.well-known/oauth-*` routes; see
  [self-hosting](../self-hosting.md#oauth).
- **A tool is missing**: the scope you approved doesn't include it (for
  example `ai:read` alone can't generate), or your role can't use it. Remove
  and re-add the plugin to approve other scopes.
- More in [troubleshooting](../troubleshooting.md#oauth).

## Checked against

<https://developers.openai.com/plugins/deploy/connect-chatgpt.md> and
<https://developers.openai.com/plugins/build/auth.md> on 2026-11-03.
Checked against docs: ChatGPT was not run. LiteGen's OAuth was run end to
end with the official MCP TypeScript SDK and with Claude Code.
