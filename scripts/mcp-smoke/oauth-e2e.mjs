#!/usr/bin/env node
// OAuth end to end with the official MCP TypeScript SDK, as ChatGPT or
// claude.ai sign in: the SDK finds LiteGen's authorization server from the
// 401, registers itself (dynamic registration), and sends a person to the
// consent page; the person approves (here through the consent page's API,
// with their dashboard session); the SDK redeems the code, calls tools,
// refreshes its access token when it expires, and is locked out once the
// grant is revoked.
//
//   LITEGEN_BASE_URL       the server (default http://127.0.0.1:4000). Run it
//                          single-tenant with a master key, password login, a
//                          fresh database, and a short access token life:
//                          LITEGEN__MCP__OAUTH__ACCESS_TOKEN_TTL_SECS=3
//   LITEGEN_E2E_EMAIL      the account to sign up, or sign in as when it exists
//   LITEGEN_E2E_PASSWORD
//   LITEGEN_E2E_TTL_WAIT_MS  how long to wait for the access token to expire
//                          (default 4000)
//
// Prints PASS and exits 0 when every step works; exits 1 otherwise.

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { UnauthorizedError } from "@modelcontextprotocol/sdk/client/auth.js";

const BASE = (process.env.LITEGEN_BASE_URL || "http://127.0.0.1:4000").replace(/\/+$/, "");
const MCP_URL = new URL(`${BASE}/mcp`);
const EMAIL = process.env.LITEGEN_E2E_EMAIL || "mcp-oauth-e2e@example.com";
const PASSWORD = process.env.LITEGEN_E2E_PASSWORD || "mcp-oauth-e2e-password-1";
const TTL_WAIT_MS = Number(process.env.LITEGEN_E2E_TTL_WAIT_MS || 4000);
/** Where a native client listens for the code: loopback, on its own port. */
const REDIRECT = "http://127.0.0.1:53111/callback";

function check(ok, message) {
  if (!ok) throw new Error(message);
}

function step(message) {
  console.log(`- ${message}`);
}

/** Keeps what a real client would keep on disk: its registration, verifier and tokens. */
class MemoryProvider {
  redirects = 0;

  get redirectUrl() {
    return REDIRECT;
  }

  get clientMetadata() {
    return {
      client_name: "LiteGen OAuth e2e",
      redirect_uris: [REDIRECT],
      grant_types: ["authorization_code", "refresh_token"],
      response_types: ["code"],
      token_endpoint_auth_method: "none",
    };
  }

  clientInformation() {
    return this.client;
  }

  saveClientInformation(client) {
    this.client = client;
  }

  tokens() {
    return this.saved;
  }

  saveTokens(tokens) {
    this.saved = tokens;
  }

  saveCodeVerifier(verifier) {
    this.verifier = verifier;
  }

  codeVerifier() {
    return this.verifier;
  }

  redirectToAuthorization(url) {
    this.redirects += 1;
    this.authorizationUrl = url;
  }
}

/** A person's dashboard session (cookie and CSRF token): sign up, or sign in when the account exists. */
async function signIn() {
  const json = { "content-type": "application/json" };
  const body = JSON.stringify({ email: EMAIL, password: PASSWORD });
  let res = await fetch(`${BASE}/v1/auth/signup`, { method: "POST", headers: json, body });
  if (!res.ok) res = await fetch(`${BASE}/v1/auth/login`, { method: "POST", headers: json, body });
  check(res.ok, `could not sign up or sign in as ${EMAIL} (${res.status}); start the server with a fresh database`);
  const cookie = res.headers.getSetCookie().map((c) => c.split(";")[0]).find((c) => c.startsWith("litegen_session="));
  check(cookie, "the sign-in set no session cookie");
  const csrf = await (await fetch(`${BASE}/v1/auth/csrf`, { headers: { cookie } })).json();
  return { cookie, csrf: csrf.csrf_token };
}

/** Play the person's part: follow the authorization URL to the consent page and approve every requested scope. */
async function approve(person, authorizationUrl) {
  const res = await fetch(authorizationUrl, { redirect: "manual" });
  if (res.status !== 302) throw new Error(`/oauth/authorize answered ${res.status}: ${await res.text()}`);
  const consent = new URL(res.headers.get("location"));
  const requestId = consent.searchParams.get("request");
  check(requestId, `no request id in ${consent}`);

  const headers = { cookie: person.cookie, "x-csrf-token": person.csrf, "content-type": "application/json" };
  const view = await (await fetch(`${BASE}/v1/oauth/requests/${encodeURIComponent(requestId)}`, { headers })).json();
  const org = view.orgs?.find((o) => o.grantable_scopes.length > 0);
  check(org, `no org to approve in: ${JSON.stringify(view)}`);
  const decided = await fetch(`${BASE}/v1/oauth/requests/${encodeURIComponent(requestId)}/approve`, {
    method: "POST",
    headers,
    body: JSON.stringify({ org_id: org.id, scopes: org.grantable_scopes }),
  });
  if (!decided.ok) throw new Error(`approve answered ${decided.status}: ${await decided.text()}`);
  const back = new URL((await decided.json()).redirect_to);
  check(back.href.startsWith(REDIRECT), `sent back to ${back.origin}${back.pathname}, not ${REDIRECT}`);
  check(back.searchParams.get("iss") === BASE, `iss is ${back.searchParams.get("iss")}, not ${BASE}`);
  return { code: back.searchParams.get("code"), scopes: org.grantable_scopes };
}

async function connect(provider) {
  const transport = new StreamableHTTPClientTransport(MCP_URL, { authProvider: provider });
  const client = new Client({ name: "litegen-oauth-e2e", version: "1.0.0" });
  await client.connect(transport);
  return client;
}

async function listModels(client) {
  const result = await client.callTool({ name: "list_models", arguments: { media_type: "image" } });
  check(!result.isError, `list_models failed: ${JSON.stringify(result.content)}`);
}

async function main() {
  const person = await signIn();
  step(`signed in to the dashboard as ${EMAIL}`);

  // A client with nothing yet: the 401 starts discovery, registration and consent.
  const provider = new MemoryProvider();
  const transport = new StreamableHTTPClientTransport(MCP_URL, { authProvider: provider });
  try {
    await new Client({ name: "litegen-oauth-e2e", version: "1.0.0" }).connect(transport);
    throw new Error("connected without signing in");
  } catch (e) {
    if (!(e instanceof UnauthorizedError)) throw e;
  }
  check(provider.authorizationUrl, "the SDK never asked for authorization");
  check(provider.client?.client_id?.startsWith("lgc_"), `the SDK did not register: ${JSON.stringify(provider.client)}`);
  step(`registered as ${provider.client.client_id}; sent to ${provider.authorizationUrl.origin}${provider.authorizationUrl.pathname}`);

  const { code, scopes } = await approve(person, provider.authorizationUrl);
  await transport.finishAuth(code);
  check(provider.saved?.access_token?.startsWith("lgat_"), "no access token after the code exchange");
  check(provider.saved?.refresh_token?.startsWith("lgrt_"), "no refresh token after the code exchange");
  step(`approved ${scopes.join(" ")}; tokens issued`);

  const client = await connect(provider);
  const { tools } = await client.listTools();
  check(tools.some((t) => t.name === "generate_image"), "generate_image is not listed");
  await listModels(client);
  step(`connected: ${tools.length} tools, list_models works`);

  const first = provider.saved.access_token;
  await new Promise((resolve) => setTimeout(resolve, TTL_WAIT_MS));
  await listModels(client);
  check(provider.saved.access_token !== first, "the call worked but the SDK never refreshed: is the access token life short?");
  step("the expired access token was refreshed and the call went through");

  const revoked = await fetch(`${BASE}/oauth/revoke`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ token: provider.saved.refresh_token }),
  });
  check(revoked.status === 200, `revoke answered ${revoked.status}`);
  let lockedOut = false;
  try {
    lockedOut = (await client.callTool({ name: "list_models", arguments: {} })).isError === true;
  } catch {
    lockedOut = true;
  }
  check(lockedOut, "the revoked grant still works");
  step("revoked: the client is locked out and must sign in again");
  console.log("PASS");
}

main().catch((e) => {
  console.error(`FAIL: ${e.message}`);
  process.exit(1);
});
