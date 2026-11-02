# APIs and SDKs

Use LiteGen's MCP server from code: OpenAI's Responses API and Agents SDK,
Anthropic's Messages API and Claude Agent SDK, and the official MCP SDKs.
Each one sends LiteGen API keys as `Authorization: Bearer …`.

Every example reads the key from `LITEGEN_API_KEY`, a key with the **AI**
preset from the dashboard's **Keys** page. For the management tools, point
the same code at `/admin/mcp` with a **Management** key. See
[permissions](../permissions.md).

**Self-hosted:** replace `https://app.litegen.ai/api/mcp` with
`{public_base_url}/mcp`. The OpenAI and Anthropic APIs connect from their
own servers, so they need a LiteGen reachable from the internet over HTTPS;
the SDKs connect from wherever your code runs.

**Timeouts.** Some image models take a minute or more, and these clients
default to short limits. Each example raises it to ten minutes. Video and 3D
generations return a job at once: pass its `id` to `wait_for_generation`,
which waits up to `timeout_seconds` (default 45, at most 230) and returns the
result or the progress so far.

**Fewer tools.** Each example can narrow the tool list: ask LiteGen for
[toolsets](../README.md#toolsets) in the URL
(`https://app.litegen.ai/api/mcp?toolsets=images,catalog`), or use the
client's own allowlist, shown below.

## OpenAI Responses API

OpenAI's servers connect to LiteGen and call its tools during the response.
Pass the key in `authorization`; OpenAI does not store it, so send it with
every request.

```python
import os
from openai import OpenAI

client = OpenAI()
response = client.responses.create(
    model="gpt-6-astra",  # any model that supports remote MCP tools
    input="Make a watercolor fox with the cheapest LiteGen image model.",
    tools=[
        {
            "type": "mcp",
            "server_label": "litegen",
            "server_url": "https://app.litegen.ai/api/mcp",
            "authorization": os.environ["LITEGEN_API_KEY"],
            "allowed_tools": ["list_models", "get_model", "estimate_image_cost", "generate_image"],
            "require_approval": "never",
        }
    ],
)
print(response.output_text)
```

By default OpenAI asks for approval (an `mcp_approval_request` output item)
before each call; `"require_approval": "never"` skips that. Keep
`allowed_tools` to what the task needs: every listed tool that generates
spends your budget.

## OpenAI Agents SDK

Python:

```python
import asyncio
import os

from agents import Agent, Runner
from agents.mcp import MCPServerStreamableHttp


async def main():
    async with MCPServerStreamableHttp(
        name="litegen",
        params={
            "url": "https://app.litegen.ai/api/mcp",
            "headers": {"Authorization": f"Bearer {os.environ['LITEGEN_API_KEY']}"},
            "timeout": 30,
            "sse_read_timeout": 600,
        },
        client_session_timeout_seconds=600,
        cache_tools_list=True,
    ) as litegen:
        agent = Agent(name="Artist", instructions="Make images with LiteGen.", mcp_servers=[litegen])
        result = await Runner.run(agent, "List LiteGen's image models.")
        print(result.final_output)


asyncio.run(main())
```

`client_session_timeout_seconds` bounds each tool call and **defaults to 5
seconds**, too short for any generation. To let OpenAI's servers call
LiteGen instead, use `HostedMCPTool` with the Responses API fields above.

TypeScript:

```ts
import { Agent, MCPServerStreamableHttp, run } from "@openai/agents";

const litegen = new MCPServerStreamableHttp({
  name: "litegen",
  url: "https://app.litegen.ai/api/mcp",
  requestInit: { headers: { Authorization: `Bearer ${process.env.LITEGEN_API_KEY}` } },
  timeout: 600_000, // milliseconds per request
  clientSessionTimeoutSeconds: 600,
  cacheToolsList: true,
});
await litegen.connect();
try {
  const agent = new Agent({ name: "Artist", instructions: "Make images with LiteGen.", mcpServers: [litegen] });
  const result = await run(agent, "List LiteGen's image models.");
  console.log(result.finalOutput);
} finally {
  await litegen.close();
}
```

Both SDKs take a `tool_filter` / `toolFilter` to expose a subset of tools.

## Anthropic Messages API (MCP connector)

Anthropic's servers connect to LiteGen and call its tools during the
message. The connector is a beta: send `mcp-client-2025-11-20`.

```python
import os
import anthropic

client = anthropic.Anthropic()
response = client.beta.messages.create(
    model="claude-opus-5-5",
    max_tokens=2000,
    betas=["mcp-client-2025-11-20"],
    messages=[{"role": "user", "content": "List LiteGen's image models."}],
    mcp_servers=[
        {
            "type": "url",
            "url": "https://app.litegen.ai/api/mcp",
            "name": "litegen",
            "authorization_token": os.environ["LITEGEN_API_KEY"],
        }
    ],
    tools=[
        {
            "type": "mcp_toolset",
            "mcp_server_name": "litegen",
            "default_config": {"enabled": False},
            "configs": {
                "list_models": {"enabled": True},
                "get_model": {"enabled": True},
                "estimate_image_cost": {"enabled": True},
                "generate_image": {"enabled": True},
            },
        }
    ],
)
print(response.content)
```

`authorization_token` is sent as `Authorization: Bearer …`; the connector
cannot send other headers. `default_config: {"enabled": False}` plus
`configs` makes an allowlist; leave both out to enable every tool. The URL
must start with `https://`.

## Claude Agent SDK

The Agent SDK runs Claude Code's agent loop in your process, with the same
MCP support and limits as [Claude Code](claude-code.md).

TypeScript:

```ts
import { query } from "@anthropic-ai/claude-agent-sdk";

for await (const message of query({
  prompt: "List LiteGen's image models.",
  options: {
    mcpServers: {
      litegen: {
        type: "http",
        url: "https://app.litegen.ai/api/mcp",
        headers: { Authorization: `Bearer ${process.env.LITEGEN_API_KEY}` },
      },
    },
    allowedTools: ["mcp__litegen__list_models", "mcp__litegen__get_model"],
    env: { ...process.env, MCP_TOOL_TIMEOUT: "600000" },
  },
})) {
  if (message.type === "result" && message.subtype === "success") console.log(message.result);
}
```

Python:

```python
import asyncio
import os

from claude_agent_sdk import ClaudeAgentOptions, ResultMessage, query


async def main():
    options = ClaudeAgentOptions(
        mcp_servers={
            "litegen": {
                "type": "http",
                "url": "https://app.litegen.ai/api/mcp",
                "headers": {"Authorization": f"Bearer {os.environ['LITEGEN_API_KEY']}"},
            }
        },
        allowed_tools=["mcp__litegen__list_models", "mcp__litegen__get_model"],
    )
    async for message in query(prompt="List LiteGen's image models.", options=options):
        if isinstance(message, ResultMessage) and message.subtype == "success":
            print(message.result)


asyncio.run(main())
```

Tools are named `mcp__litegen__<tool>`. Only the tools in
`allowedTools` / `allowed_tools` run without a permission prompt;
`mcp__litegen__*` allows them all, including the ones that spend your
budget. `MCP_TOOL_TIMEOUT=600000` (milliseconds) in the agent's environment
raises Claude Code's 60-second per-request limit.

## MCP TypeScript SDK

With `@modelcontextprotocol/sdk` (1.x):

```js
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const transport = new StreamableHTTPClientTransport(new URL("https://app.litegen.ai/api/mcp"), {
  requestInit: { headers: { Authorization: `Bearer ${process.env.LITEGEN_API_KEY}` } },
});
const client = new Client({ name: "my-app", version: "1.0.0" });
await client.connect(transport);

const { tools } = await client.listTools();
const models = await client.callTool({ name: "list_models", arguments: { media_type: "image" } });
console.log(tools.length, models.structuredContent.data.length);

// Requests time out after 60 s by default; give generation ten minutes.
const image = await client.callTool(
  { name: "generate_image", arguments: { model: "<model id from list_models>", prompt: "A red lighthouse at dusk" } },
  undefined,
  { timeout: 600_000 },
);
await client.close();
```

Results carry `structuredContent` (LiteGen's JSON response) and the same JSON
as text. A failed call has `isError: true` and text such as
`HTTP 402 budget_exceeded: …`. [`scripts/mcp-smoke`](../../../scripts/mcp-smoke/README.md)
is a runnable version of this.

## MCP Python SDK

With `mcp` 2.x (headers go on the `httpx2` client the transport uses):

```python
import asyncio
import os

import httpx2
from mcp import Client
from mcp.client.streamable_http import streamable_http_client


async def main():
    http = httpx2.AsyncClient(
        headers={"Authorization": f"Bearer {os.environ['LITEGEN_API_KEY']}"},
        timeout=httpx2.Timeout(30, read=600),
    )
    async with http, Client(streamable_http_client("https://app.litegen.ai/api/mcp", http_client=http)) as client:
        tools = await client.list_tools()
        models = await client.call_tool("list_models", {"media_type": "image"})
        print(len(tools.tools), len(models.structured_content["data"]))


asyncio.run(main())
```

`call_tool` takes `read_timeout_seconds=` for a slow generation, and
`progress_callback=` to follow `wait_for_generation`'s progress.

## Checked against

On 2026-11-02:
<https://developers.openai.com/api/docs/guides/tools-connectors-mcp>,
<https://openai.github.io/openai-agents-python/mcp/>,
the OpenAI Agents JS guide (github.com/openai/openai-agents-js
`docs/src/content/docs/guides/mcp.mdx`),
<https://platform.claude.com/docs/en/agents-and-tools/mcp-connector>,
<https://code.claude.com/docs/en/agent-sdk/mcp>,
and the MCP SDKs' own source. The MCP TypeScript SDK (1.32.2) and Python
SDK (2.3.0) examples were run against a local LiteGen server: both listed
21 tools and 99 image models with an AI key. The OpenAI and Anthropic
examples were checked against docs only; their servers cannot reach a local
LiteGen.
