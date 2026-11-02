#!/usr/bin/env node
// Smoke-test a LiteGen MCP endpoint with the official MCP TypeScript SDK.
//
//   LITEGEN_MCP_URL   endpoint (default http://127.0.0.1:4000/mcp)
//   LITEGEN_API_KEY   sent as "Authorization: Bearer <key>" when set
//
//   node smoke.mjs                     connect, list tools, call list_models
//                                      (get_my_limits on /admin/mcp)
//   node smoke.mjs --generate <model>  also call generate_image with <model>
//
// Exits 0 when every step succeeds, 1 on any failure.

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const DEFAULT_URL = "http://127.0.0.1:4000/mcp";
const PROMPT = "A red lighthouse on a cliff at dusk";
const GENERATE_TIMEOUT_MS = 600_000;
const USAGE = "usage: node smoke.mjs [--generate <image model id>]";

function parseArgs(argv) {
  const args = { generate: null };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--generate") {
      args.generate = argv[++i];
      if (!args.generate || args.generate.startsWith("--")) throw new Error(`--generate needs a model id\n${USAGE}`);
    } else if (arg === "--help" || arg === "-h") {
      console.log(USAGE);
      process.exit(0);
    } else {
      throw new Error(`unknown argument '${arg}'\n${USAGE}`);
    }
  }
  return args;
}

function textOf(result) {
  return (result.content ?? [])
    .filter((block) => block.type === "text")
    .map((block) => block.text)
    .join("\n");
}

async function call(client, name, args, options) {
  const result = await client.callTool({ name, arguments: args }, undefined, options);
  if (result.isError) throw new Error(`${name} returned a tool error: ${textOf(result)}`);
  return result;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const url = process.env.LITEGEN_MCP_URL || DEFAULT_URL;
  const key = process.env.LITEGEN_API_KEY;
  const headers = key ? { Authorization: `Bearer ${key}` } : {};

  const transport = new StreamableHTTPClientTransport(new URL(url), { requestInit: { headers } });
  const client = new Client({ name: "litegen-mcp-smoke", version: "1.0.0" });
  await client.connect(transport);

  try {
    const server = client.getServerVersion();
    console.log(`connected  ${url} (${server?.name ?? "unknown"} ${server?.version ?? ""})`.trimEnd());

    const { tools } = await client.listTools();
    console.log(`tools      ${tools.length}`);
    if (tools.length === 0) throw new Error("the endpoint lists no tools for this credential");

    const names = new Set(tools.map((tool) => tool.name));
    if (names.has("list_models")) {
      const listed = await call(client, "list_models", { media_type: "image" });
      const models = listed.structuredContent?.data;
      if (!Array.isArray(models)) throw new Error(`list_models returned no data array: ${textOf(listed).slice(0, 300)}`);
      console.log(`models     ${models.length} image models${models.length ? `, e.g. ${models[0].id}` : ""}`);
    } else if (names.has("get_my_limits")) {
      // /admin/mcp serves no list_models; get_my_limits is read-only and on both endpoints.
      await call(client, "get_my_limits", {});
      console.log("limits     get_my_limits ok");
    } else {
      throw new Error("the endpoint lists neither list_models nor get_my_limits");
    }

    if (args.generate) {
      if (!names.has("generate_image")) throw new Error("--generate needs generate_image, which this endpoint does not list for this key");
      // Some image models take minutes; the SDK's default request timeout is 60 s.
      const generated = await call(client, "generate_image", { model: args.generate, prompt: PROMPT }, { timeout: GENERATE_TIMEOUT_MS });
      // Images come back as URLs, or inline as b64_json when the server stores them inline.
      const images = generated.structuredContent?.data ?? [];
      const previews = generated.content.filter((block) => block.type === "image").length;
      if (images.length === 0) throw new Error(`generate_image returned no images: ${textOf(generated).slice(0, 300)}`);
      console.log(`generated  ${images.length} image(s) with ${args.generate}, ${previews} preview(s)`);
      for (const { url: imageUrl } of images) {
        if (imageUrl) console.log(`           ${imageUrl.length > 120 ? `${imageUrl.slice(0, 117)}...` : imageUrl}`);
      }
    }
  } finally {
    await client.close();
  }
  console.log("ok");
}

main().catch((err) => {
  console.error(`smoke failed: ${err?.message ?? err}`);
  process.exit(1);
});
