# Test hook: behaves as the request's x-test-action / x-test-post / x-test-file
# headers say.
# behaviour.mjs and behaviour.ts do exactly the same, so every test runs
# unchanged in every runtime. See "Test fixtures" in
# docs/superpowers/plans/2026-11-02-generation-call-hooks.md.
import asyncio
import json
import os
import sys

LABEL = os.environ.get("HOOK_LABEL", "python")


class HTTPError(Exception):
    """FastAPI's HTTPException shape: status_code, detail, headers."""

    def __init__(self, status_code, detail, headers=None):
        super().__init__(detail)
        self.status_code = status_code
        self.detail = detail
        self.headers = headers or {}


def _actions(headers, key):
    raw = (headers or {}).get(key, "")
    return [a.strip() for a in raw.split(",") if a.strip()]


async def pre_call(request):
    changed = False
    headers = request.get("headers") or {}
    for action in _actions(headers, "x-test-action"):
        if action == "inject":
            request["headers"]["authorization"] = "Bearer " + os.environ.get("TEST_LITEGEN_KEY", "")
            changed = True
        elif action == "refuse-status":
            raise HTTPError(401, "who are you", {"www-authenticate": "Test"})
        elif action == "refuse-string":
            return "not today"
        elif action == "rewrite-prompt":
            request["body"]["prompt"] = request["body"]["prompt"] + " (hooked)"
            changed = True
        elif action == "n2":
            request["body"]["n"] = 2
            changed = True
        elif action == "sleep":
            await asyncio.sleep(3)
        elif action == "sleep-300":
            await asyncio.sleep(0.3)
        elif action == "echo-id":
            request["headers"]["x-answer"] = headers.get("x-id", "")
            changed = True
        elif action == "crash":
            os._exit(3)
        elif action == "hang":
            while True:  # blocks the event loop: no call is answered again
                pass
        elif action == "print":
            # No newline: leaked onto the protocol, it would corrupt the next answer.
            print("noise", end="")
            sys.stdout.flush()
        elif action == "bad-shape":
            return 42
        elif action == "change-path":
            request["path"] = "/v1/keys"
            changed = True
        elif action == "report":
            seen = {k: request.get(k) for k in ("body", "files", "query", "headers")}
            raise HTTPError(418, json.dumps(seen))
        elif action == "env":
            inherited = os.environ.get("CARGO_MANIFEST_DIR", "")
            own = os.environ.get("HOOK_OWN_VAR", "")
            raise HTTPError(418, f"inherited={inherited} own={own}")
    return request if changed else None


def post_call(request, response, caller):
    changed = False
    for action in _actions(request.get("headers"), "x-test-post"):
        if action == "tag":
            if isinstance(response.get("body"), dict):
                response["body"]["hooked"] = LABEL
            response["headers"]["x-hooked"] = LABEL
            changed = True
        elif action == "fail":
            raise RuntimeError("boom")
        elif action == "report":
            response["headers"]["x-seen-caller"] = json.dumps(caller)
            changed = True
        elif action == "teapot":
            response["status"] = 418
            changed = True
    return response if changed else None


def file_access(request, file, caller):
    for action in _actions(request.get("headers"), "x-test-file"):
        if action == "deny-other-end-user":
            mine = (request.get("headers") or {}).get("x-litegen-end-user")
            if file["created_by"].get("end_user") != mine:
                raise HTTPError(403, "not your file")
        elif action == "report":
            raise HTTPError(418, json.dumps({"file": file, "caller": caller}))
        elif action == "fail":
            raise RuntimeError("boom")
        elif action == "deny-string":
            return "files are closed today"
        elif action == "deny-false":
            return False
    return None
