# litegen's Python hook runner. litegen starts it as
#   python3 runner.py <hook file>
# and talks to it in JSON lines: one "ready" line from the runner, then calls
#   {"id": 7, "fn": "pre_call", "args": [...]}
# answered, in any order, by one of
#   {"id": 7, "result": <value or null>}
#   {"id": 7, "refuse": {"status": 401, "message": "...", "headers": {...}}}
#   {"id": 7, "error": "<message and traceback>"}
# The runner exits when its stdin closes, so it never outlives litegen.
import asyncio
import importlib.util
import inspect
import json
import os
import sys
import threading
import traceback

HOOKS = ("pre_call", "post_call", "file_access")


def main():
    hook_path = os.path.abspath(sys.argv[1])
    # The hook's directory is the operator's: no __pycache__ beside the file.
    sys.dont_write_bytecode = True

    # The protocol keeps a private copy of stdout. Everything else written to
    # stdout, by print() or by native code, goes to stderr, which litegen
    # logs, so a hook's own output can never corrupt an answer.
    proto = os.fdopen(os.dup(1), "w", encoding="utf-8")
    os.dup2(2, 1)
    sys.stdout = sys.stderr
    lock = threading.Lock()

    def send(message):
        line = json.dumps(message, separators=(",", ":"), ensure_ascii=False)
        with lock:
            proto.write(line + "\n")
            proto.flush()

    try:
        sys.path.insert(0, os.path.dirname(hook_path))
        spec = importlib.util.spec_from_file_location("litegen_hook", hook_path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
    except BaseException as e:  # a SyntaxError, a failed import, a raise at import
        send({"ready": False, "error": f"{type(e).__name__}: {e}\n{traceback.format_exc()}"})
        return 1

    defined = [name for name in HOOKS if callable(getattr(module, name, None))]
    if not defined:
        send({"ready": False, "error": f"{os.path.basename(hook_path)} defines none of pre_call, post_call and file_access"})
        return 1
    send({"ready": True, "hooks": defined})
    asyncio.run(serve(module, send))
    return 0


async def serve(module, send):
    loop = asyncio.get_running_loop()
    lines = asyncio.Queue()

    def read_stdin():
        for raw in sys.stdin.buffer:
            loop.call_soon_threadsafe(lines.put_nowait, raw)
        loop.call_soon_threadsafe(lines.put_nowait, None)

    threading.Thread(target=read_stdin, daemon=True).start()
    tasks = set()
    while True:
        raw = await lines.get()
        if raw is None:
            return
        if not raw.strip():
            continue
        task = asyncio.create_task(answer(module, json.loads(raw), send))
        tasks.add(task)
        task.add_done_callback(tasks.discard)


async def answer(module, call, send):
    call_id = call.get("id")
    try:
        fn = getattr(module, call["fn"])
        if inspect.iscoroutinefunction(fn):
            result = await fn(*call["args"])
        else:
            result = await asyncio.to_thread(fn, *call["args"])
        if inspect.isawaitable(result):
            result = await result
        if isinstance(result, str):
            # As LiteLLM answers a string from a non-chat hook: a 400. A file
            # the hook won't open is a 403.
            status = 403 if call["fn"] == "file_access" else 400
            send({"id": call_id, "refuse": {"status": status, "message": result, "headers": {}}})
            return
        json.dumps(result)  # refuse to send what JSON can't carry
        send({"id": call_id, "result": result})
    except BaseException as e:
        status = getattr(e, "status_code", None)
        if isinstance(status, int) and not isinstance(status, bool):
            detail = getattr(e, "detail", None)
            message = str(detail) if detail is not None else str(e)
            headers = getattr(e, "headers", None) or {}
            send({"id": call_id, "refuse": {
                "status": status,
                "message": message,
                "headers": {str(k): str(v) for k, v in dict(headers).items()},
            }})
        else:
            send({"id": call_id, "error": f"{type(e).__name__}: {e}\n{traceback.format_exc()}"})


if __name__ == "__main__":
    sys.exit(main())
