"""Integration-test harness: boot a real litegen-core binary (sqlite + mock
provider, hosted mode, master key + secrets key) and expose fixtures that drive
it through the GENERATED SDK in all three auth modes (master key, owner session,
API key).

Skips cleanly if the debug binary isn't built. Build it with:
    cargo build --manifest-path litegen-core/Cargo.toml
"""
from __future__ import annotations

import base64
import os
import pathlib
import socket
import subprocess
import tempfile
import time
import uuid

import httpx
import pytest

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
BINARY = REPO_ROOT / "litegen-core" / "target" / "debug" / "litegen"
MODELS_DIR = REPO_ROOT / "models"
MASTER_KEY = "test-master-key-integration-0001"
# decode_secrets_key expects base64 of exactly 32 bytes.
SECRETS_KEY = base64.b64encode(bytes(range(32))).decode()
PASSWORD = "hunter2-integration-pw"


def _free_port() -> int:
    s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


@pytest.fixture(scope="session")
def base_url():
    if not BINARY.exists():
        pytest.skip(
            f"litegen binary not built at {BINARY}; "
            f"run `cargo build --manifest-path litegen-core/Cargo.toml` first"
        )
    port = _free_port()
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="litegen-it-"))
    (tmp / "litegen.yaml").write_text(
        f'server:\n  host: "127.0.0.1"\n  port: {port}\n'
        f'database_url: "sqlite://{tmp}/it.db?mode=rwc"\n'
        f'models_dir: "{MODELS_DIR}"\n'
        f"providers:\n  mock: {{}}\n"
    )
    env = {
        **os.environ,
        "LITEGEN__MODE": "hosted",
        "LITEGEN__MASTER_KEY": MASTER_KEY,
        "LITEGEN__SECRETS_KEY": SECRETS_KEY,
        # Session cookies are Secure by default; the harness serves plain HTTP,
        # so allow non-Secure cookies or httpx won't send them back.
        "LITEGEN__COOKIE_INSECURE_DEV": "true",
        "LITEGEN_MODELS_DIR": str(MODELS_DIR),
        "RUST_LOG": "warn",
    }
    proc = subprocess.Popen(
        [str(BINARY)], cwd=str(tmp), env=env,
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    url = f"http://127.0.0.1:{port}"
    deadline = time.time() + 30
    while time.time() < deadline:
        if proc.poll() is not None:
            err = proc.stderr.read().decode() if proc.stderr else ""
            raise RuntimeError(f"litegen exited early (code {proc.returncode}):\n{err}")
        try:
            if httpx.get(f"{url}/health/live", timeout=1.0).status_code == 200:
                break
        except Exception:
            pass
        time.sleep(0.3)
    else:
        proc.terminate()
        raise RuntimeError("litegen did not become live within 30s")
    yield url
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except Exception:
        proc.kill()


def unique_email(tag: str = "it") -> str:
    return f"{tag}-{uuid.uuid4().hex[:12]}@example.com"


@pytest.fixture(scope="session")
def master_client(base_url):
    """Generated AuthenticatedClient bound to the platform-admin master key."""
    from litegen._generated.client import AuthenticatedClient

    return AuthenticatedClient(base_url=base_url, token=MASTER_KEY)


class Owner:
    """A freshly signed-up org owner, exposed in every auth mode the SDK supports."""

    def __init__(self, *, session, bearer, http, org_id, app_id, email, api_key):
        self.session = session      # generated Client carrying the owner session cookie + CSRF
        self.bearer = bearer        # generated AuthenticatedClient carrying an org API key
        self.http = http            # raw httpx.Client (cookie + CSRF) — setup / escape hatch
        self.org_id = org_id
        self.app_id = app_id
        self.email = email
        self.api_key = api_key


@pytest.fixture
def owner(base_url):
    """Sign up a fresh hosted owner and return clients for session, Bearer, and raw access.

    The session auth dance (signup → cookie → CSRF) is done with raw httpx, then
    injected into a generated `Client` via `set_httpx_client`, so generated endpoint
    calls run authenticated as the owner.
    """
    from litegen._generated.client import AuthenticatedClient, Client

    hc = httpx.Client(base_url=base_url, follow_redirects=False, timeout=30)
    email = unique_email("owner")
    r = hc.post("/v1/auth/signup", json={"email": email, "password": PASSWORD, "org_name": "IT Org"})
    assert r.status_code == 200, f"signup failed: {r.status_code} {r.text}"
    cr = hc.get("/v1/auth/csrf")
    assert cr.status_code == 200, f"csrf failed: {cr.status_code} {cr.text}"
    hc.headers["x-csrf-token"] = cr.json()["csrf_token"]

    org_id = hc.get("/v1/orgs").json()[0]["id"]
    app_id = hc.get(f"/v1/orgs/{org_id}/apps").json()[0]["id"]

    key_resp = hc.post("/v1/keys", json={"name": "it-key", "scopes": "generate,read,admin"})
    assert key_resp.status_code in (200, 201), f"create key failed: {key_resp.status_code} {key_resp.text}"
    api_key = key_resp.json()["key"]

    session = Client(base_url=base_url).set_httpx_client(hc)
    bearer = AuthenticatedClient(base_url=base_url, token=api_key)
    try:
        yield Owner(session=session, bearer=bearer, http=hc, org_id=org_id,
                    app_id=app_id, email=email, api_key=api_key)
    finally:
        hc.close()
