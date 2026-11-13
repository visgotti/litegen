// Test hook: behaves as the request's x-test-action / x-test-post / x-test-file headers say,
// exactly as behaviour.py and behaviour.ts do. Runs under Node and Bun.
const LABEL = process.env.HOOK_LABEL ?? "node";

function refuse(status, message, headers = {}) {
  const e = new Error(message);
  e.status = status;
  e.headers = headers;
  return e;
}

const actions = (headers, key) => (headers?.[key] ?? "").split(",").map((a) => a.trim()).filter(Boolean);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export async function pre_call(request) {
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
        request.body.prompt = `${request.body.prompt} (hooked)`;
        changed = true;
        break;
      case "n2":
        request.body.n = 2;
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

export function post_call(request, response, caller) {
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

export function file_access(request, file, caller) {
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
