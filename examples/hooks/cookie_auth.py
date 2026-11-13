"""Cookie auth for litegen, for browsers: your session cookie in, a litegen key injected.

A browser on your site sends its `acme_session` cookie straight to litegen.
A cookie travels with cross-site requests too, so before trusting it the hook
checks `Origin` against ALLOWED_ORIGINS. litegen leaves that CSRF check to a
hook that authenticates from a cookie. ACME_TOKENS maps session tokens to
users (`token:user,token:user`).

    hooks:
      - path: hooks/cookie_auth.py
"""
import os
from http.cookies import SimpleCookie

ALLOWED_ORIGINS = {o.strip() for o in os.environ.get("ALLOWED_ORIGINS", "").split(",") if o.strip()}
TOKENS = dict(
    pair.strip().split(":", 1)
    for pair in os.environ.get("ACME_TOKENS", "").split(",")
    if ":" in pair
)


class Refused(Exception):
    """Refuses the request with `status_code` (FastAPI's HTTPException shape)."""

    def __init__(self, status_code, detail):
        super().__init__(detail)
        self.status_code = status_code
        self.detail = detail


def pre_call(request):
    headers = request["headers"]
    if "authorization" in headers:
        return None  # already a litegen caller
    origin = headers.get("origin")
    if origin and origin not in ALLOWED_ORIGINS:
        raise Refused(403, f"cross-site request from {origin} refused")
    session = SimpleCookie(headers.get("cookie", "")).get("acme_session")
    user = TOKENS.get(session.value) if session else None
    if user is None:
        raise Refused(401, "sign in first")
    headers["authorization"] = "Bearer " + os.environ["LITEGEN_APP_KEY"]
    headers["x-litegen-end-user"] = user
    return request
