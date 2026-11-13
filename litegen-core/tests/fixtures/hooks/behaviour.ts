// Test hook in TypeScript: behaves exactly as behaviour.py and behaviour.mjs.
// Runs under Bun, and under Node 22.18+ (which strips the types itself).
type Headers = Record<string, string>;
interface HookRequest {
  method: string;
  path: string;
  query: string;
  headers: Headers;
  body: Record<string, unknown> | null;
  files: Array<{ name: string; filename?: string; content_type?: string; size: number }>;
}
interface HookResponse {
  status: number;
  headers: Headers;
  body: Record<string, unknown> | null;
}

const LABEL: string = process.env.HOOK_LABEL ?? "bun-ts";

function refuse(status: number, message: string, headers: Headers = {}): Error {
  const e = new Error(message) as Error & { status: number; headers: Headers };
  e.status = status;
  e.headers = headers;
  return e;
}

const actions = (headers: Headers | undefined, key: string): string[] =>
  (headers?.[key] ?? "").split(",").map((a) => a.trim()).filter(Boolean);
const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

export async function pre_call(request: HookRequest): Promise<HookRequest | string | number | undefined> {
  let changed = false;
  const headers = request.headers ?? {};
  for (const action of actions(headers, "x-test-action")) {
    switch (action) {
      case "inject":
        request.headers.authorization = `Bearer ${process.env.TEST_LITEGEN_KEY ?? ""}`;
        changed = true;
        break;
      case "refuse-status":
        throw refuse(401, "who are you", { "www-authenticate": "Test" });
      case "refuse-string":
        return "not today";
      case "rewrite-prompt":
        request.body!.prompt = `${request.body!.prompt} (hooked)`;
        changed = true;
        break;
      case "n2":
        request.body!.n = 2;
        changed = true;
        break;
      case "sleep":
        await sleep(3000);
        break;
      case "sleep-300":
        await sleep(300);
        break;
      case "echo-id":
        request.headers["x-answer"] = headers["x-id"] ?? "";
        changed = true;
        break;
      case "crash":
        process.exit(3);
      case "hang":
        for (;;) {} // blocks the event loop: no call is answered again
      case "print":
        // No newline: leaked onto the protocol, it would corrupt the next answer.
        process.stdout.write("noise");
        console.log("more noise");
        break;
      case "bad-shape":
        return 42;
      case "change-path":
        request.path = "/v1/keys";
        changed = true;
        break;
      case "report": {
        const { body, files, query } = request;
        throw refuse(418, JSON.stringify({ body, files, query, headers }));
      }
      case "env":
        throw refuse(418, `inherited=${process.env.CARGO_MANIFEST_DIR ?? ""} own=${process.env.HOOK_OWN_VAR ?? ""}`);
    }
  }
  return changed ? request : undefined;
}

export function post_call(request: HookRequest, response: HookResponse, caller: Record<string, unknown>): HookResponse | undefined {
  let changed = false;
  for (const action of actions(request.headers, "x-test-post")) {
    switch (action) {
      case "tag":
        if (response.body && typeof response.body === "object") response.body.hooked = LABEL;
        response.headers["x-hooked"] = LABEL;
        changed = true;
        break;
      case "fail":
        throw new Error("boom");
      case "report":
        response.headers["x-seen-caller"] = JSON.stringify(caller);
        changed = true;
        break;
      case "teapot":
        response.status = 418;
        changed = true;
        break;
    }
  }
  return changed ? response : undefined;
}

interface StoredFile {
  generation_id: string;
  index: number;
  kind: string;
  format: string;
  created_by: { key_id: string | null; app_id: string | null; org_id: string | null; end_user: string | null };
  [field: string]: unknown;
}

export function file_access(request: HookRequest, file: StoredFile, caller: Record<string, unknown>): string | boolean | undefined {
  for (const action of actions(request.headers, "x-test-file")) {
    switch (action) {
      case "deny-other-end-user":
        if (file.created_by.end_user !== (request.headers ?? {})["x-litegen-end-user"]) throw refuse(403, "not your file");
        break;
      case "report":
        throw refuse(418, JSON.stringify({ file, caller, request: { method: request.method, path: request.path } }));
      case "fail":
        throw new Error("boom");
      case "deny-string":
        return "files are closed today";
      case "deny-false":
        return false;
    }
  }
  return undefined;
}
