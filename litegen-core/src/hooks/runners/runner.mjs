// litegen's JavaScript hook runner, for Node and Bun (and TypeScript, which
// Bun runs directly and Node 22.18+ strips itself). litegen starts it as
//   node runner.mjs <hook file>     or     bun runner.mjs <hook file>
// and talks to it in JSON lines, exactly as runner.py does: one "ready" line
// from the runner, then calls
//   {"id": 7, "fn": "pre_call", "args": [...]}
// answered, in any order, by one of
//   {"id": 7, "result": <value or null>}
//   {"id": 7, "refuse": {"status": 401, "message": "...", "headers": {...}}}
//   {"id": 7, "error": "<message and stack>"}
// The runner exits when its stdin closes, so it never outlives litegen.
import path from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";

const HOOKS = ["pre_call", "post_call", "file_access"];
const hookPath = path.resolve(process.argv[2]);

// The protocol keeps the real stdout writer. Everything else written to
// stdout goes to stderr, which litegen logs, so a hook's own output can never
// corrupt an answer.
const protocolWrite = process.stdout.write.bind(process.stdout);
process.stdout.write = (chunk, encoding, callback) => process.stderr.write(chunk, encoding, callback);
for (const method of ["log", "info", "debug"]) console[method] = (...args) => console.error(...args);

function send(message) {
  protocolWrite(JSON.stringify(message) + "\n");
}

function describe(e) {
  const name = e?.name ?? "Error";
  const message = e?.message ?? String(e);
  // Bun reports a file that won't parse as an AggregateError whose `errors`
  // carry the parser's own messages.
  const nested = Array.isArray(e?.errors) ? e.errors.map((n) => `\n  ${n?.name ?? "Error"}: ${n?.message ?? String(n)}`).join("") : "";
  return `${name}: ${message}${nested}\n${e?.stack ?? ""}`;
}

async function main() {
  let module;
  try {
    module = await import(pathToFileURL(hookPath).href);
  } catch (e) {
    send({ ready: false, error: describe(e) });
    process.exitCode = 1; // exit once the answer is written, not before
    return;
  }
  // Named exports, or a CommonJS file's module.exports.
  const exportsOf = (m) => (HOOKS.some((h) => typeof m?.[h] === "function") ? m : m?.default);
  const api = exportsOf(module) ?? {};
  const defined = HOOKS.filter((h) => typeof api[h] === "function");
  if (defined.length === 0) {
    send({ ready: false, error: `${path.basename(hookPath)} defines none of pre_call, post_call and file_access` });
    process.exitCode = 1;
    return;
  }
  send({ ready: true, hooks: defined });

  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  lines.on("line", (line) => {
    if (!line.trim()) return;
    let call;
    try {
      call = JSON.parse(line);
    } catch {
      return;
    }
    answer(api, call);
  });
  lines.on("close", () => process.exit(0));
}

async function answer(api, call) {
  const id = call.id;
  try {
    let result = await api[call.fn](...call.args);
    if (typeof result === "string") {
      // As LiteLLM answers a string from a non-chat hook: a 400. A file the
      // hook won't open is a 403.
      send({ id, refuse: { status: call.fn === "file_access" ? 403 : 400, message: result, headers: {} } });
      return;
    }
    send({ id, result: result === undefined ? null : result });
  } catch (e) {
    const status = e?.status ?? e?.statusCode;
    if (Number.isInteger(status)) {
      const headers = Object.fromEntries(Object.entries(e.headers ?? {}).map(([k, v]) => [String(k), String(v)]));
      send({ id, refuse: { status, message: String(e.message ?? ""), headers } });
    } else {
      send({ id, error: describe(e) });
    }
  }
}

main();
