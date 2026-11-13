"""Custom auth for litegen: callers send your token, the hook injects a litegen key.

Callers send `x-acme-token: <token>` instead of a litegen API key. ACME_TOKENS
maps tokens to users (`token:user,token:user`). A known token gets litegen's
key (LITEGEN_APP_KEY) and is recorded as that end user, so per-user budgets
and spend apply. A request that already carries a litegen key is left to
litegen's own auth.

    hooks:
      - path: hooks/custom_auth.py
"""
import os


class Unauthorized(Exception):
    """Refuses the request: FastAPI's HTTPException shape, as LiteLLM uses."""

    status_code = 401

    def __init__(self, detail):
        super().__init__(detail)
        self.detail = detail
        self.headers = {"www-authenticate": "AcmeToken"}


TOKENS = dict(
    pair.strip().split(":", 1)
    for pair in os.environ.get("ACME_TOKENS", "").split(",")
    if ":" in pair
)


def pre_call(request):
    headers = request["headers"]
    if "authorization" in headers:
        return None  # already a litegen caller
    user = TOKENS.get(headers.get("x-acme-token", ""))
    if user is None:
        raise Unauthorized("send a valid x-acme-token")
    headers["authorization"] = "Bearer " + os.environ["LITEGEN_APP_KEY"]
    headers["x-litegen-end-user"] = user
    del headers["x-acme-token"]  # litegen doesn't need it
    return request
