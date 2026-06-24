"""Validates the integration harness itself: the booted binary serves the core
flows (health, models w/ mock, hosted signup + session, mock generation) before
the exhaustive SDK suite layers on top. Raw httpx — no SDK — to isolate harness
issues from SDK issues.
"""
import uuid

import httpx

MASTER_KEY = "test-master-key-integration-0001"
PASSWORD = "hunter2-integration-pw"


def test_boot_and_core_flow(base_url):
    # Liveness + readiness (readiness must be 200 now that it gates on DB only).
    assert httpx.get(f"{base_url}/health/live", timeout=5).status_code == 200
    assert httpx.get(f"{base_url}/health/ready", timeout=5).status_code == 200

    h = {"authorization": f"Bearer {MASTER_KEY}"}

    # Models list includes the mock provider's models.
    r = httpx.get(f"{base_url}/v1/models", headers=h, timeout=10)
    assert r.status_code == 200, r.text
    assert "mock/image-gen" in r.text, "mock models should be registered"

    # Hosted signup creates a user + org + session cookie.
    c = httpx.Client(base_url=base_url, follow_redirects=False, timeout=10)
    email = f"smoke-{uuid.uuid4().hex[:10]}@example.com"
    r = c.post("/v1/auth/signup", json={"email": email, "password": PASSWORD, "org_name": "Smoke"})
    assert r.status_code == 200, r.text
    assert "litegen_session" in c.cookies, "signup must set the session cookie"

    cr = c.get("/v1/auth/csrf")
    assert cr.status_code == 200, f"csrf failed (session cookie not sent?): {cr.status_code} {cr.text}"
    c.headers["x-csrf-token"] = cr.json()["csrf_token"]

    orgs = c.get("/v1/orgs")
    assert orgs.status_code == 200, orgs.text
    assert len(orgs.json()) >= 1, "signup should create one org"

    # Mock image generation via the master key (deterministic, no network).
    r = httpx.post(
        f"{base_url}/v1/images/generations",
        headers=h,
        json={"model": "mock/image-gen", "prompt": "hello"},
        timeout=30,
    )
    assert r.status_code == 200, r.text
