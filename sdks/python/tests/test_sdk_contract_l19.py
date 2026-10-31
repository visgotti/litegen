"""L19: a versioned SDK that shows response headers and waits as long as the
server can take."""

from __future__ import annotations

import inspect
import pathlib
import re

import httpx
import pytest
import respx
from httpx import Response

import litegen
from litegen import AsyncLiteGenClient, LiteGenAPIError, LiteGenClient, polling
from litegen.async_client import _AsyncModels3d, _AsyncVideos
from litegen.client import DEFAULT_TIMEOUT, _Models3dNamespace, _VideosNamespace, response_meta
from litegen.polling import JOB_DEFAULT_TIMEOUT, MODEL3D_DEFAULT_TIMEOUT

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASE = "http://localhost:4000"
SENT = {
    "x-litegen-dropped-params": "seed, negative_prompt",
    "x-ratelimit-limit": "60",
    "x-ratelimit-remaining": "59",
    "x-ratelimit-reset": "1793404800",
    "idempotent-replayed": "true",
}


def test_one_semantic_version_in_pyproject_the_code_and_the_changelog() -> None:
    found = re.search(r'^version = "(\d+\.\d+\.\d+)"', (ROOT / "pyproject.toml").read_text(), re.M)
    assert found, "pyproject.toml names a semantic version"
    assert litegen.__version__ == found.group(1)
    latest = re.search(r"^## \[(\d+\.\d+\.\d+)\]", (ROOT / "CHANGELOG.md").read_text(), re.M)
    assert latest and latest.group(1) == found.group(1)


def test_response_meta_reads_litegens_headers() -> None:
    assert response_meta(httpx.Headers(SENT)) == {
        "dropped_params": ["seed", "negative_prompt"],
        "rate_limit": {"limit": 60, "remaining": 59, "reset_at": 1793404800},
        "idempotent_replayed": True,
        "retry_after": None,
    }
    assert response_meta(httpx.Headers()) == {
        "dropped_params": [],
        "rate_limit": None,
        "idempotent_replayed": False,
        "retry_after": None,
    }


@respx.mock
def test_every_response_reaches_on_response_and_an_errors_headers_reach_the_error() -> None:
    respx.get(f"{BASE}/health/live").mock(
        side_effect=[
            Response(200, json={"status": "ok"}, headers=SENT),
            Response(
                429,
                json={"error": {"message": "slow down", "type": "rate_limit_exceeded", "code": "rate_limit_exceeded"}},
                headers={"retry-after": "7", "x-ratelimit-limit": "60", "x-ratelimit-remaining": "0"},
            ),
        ]
    )
    seen: list[httpx.Response] = []
    client = LiteGenClient(base_url=BASE, on_response=seen.append)
    client.health.live()
    with pytest.raises(LiteGenAPIError) as exc:
        client.health.live()
    assert [(r.request.method, r.url.path, r.status_code) for r in seen] == [
        ("GET", "/health/live", 200),
        ("GET", "/health/live", 429),
    ]
    assert response_meta(seen[0].headers)["dropped_params"] == ["seed", "negative_prompt"]
    assert exc.value.headers is not None and exc.value.headers["x-ratelimit-remaining"] == "0"
    assert exc.value.retry_after == 7


@respx.mock
async def test_the_async_client_calls_on_response_too() -> None:
    respx.get(f"{BASE}/health/live").mock(return_value=Response(200, json={"status": "ok"}, headers=SENT))
    seen: list[httpx.Response] = []
    async with AsyncLiteGenClient(base_url=BASE, on_response=seen.append) as client:
        await client.health.live()
    assert [r.status_code for r in seen] == [200]


def test_a_request_waits_out_the_servers_worst_case() -> None:
    # A synchronous call can take three vendor attempts of up to 120 s each.
    assert DEFAULT_TIMEOUT > 3 * 120
    assert LiteGenClient(base_url=BASE)._http.timeout.read == DEFAULT_TIMEOUT
    assert AsyncLiteGenClient(base_url=BASE)._http.timeout.read == DEFAULT_TIMEOUT


def test_polling_waits_past_the_servers_reap_age() -> None:
    # The server fails a job still running after 2 hours itself.
    assert JOB_DEFAULT_TIMEOUT > 2 * 60 * 60
    assert MODEL3D_DEFAULT_TIMEOUT == JOB_DEFAULT_TIMEOUT
    pollers = [getattr(polling, name) for name in dir(polling) if name.startswith(("poll", "apoll", "wait_for", "async_wait_for"))]
    methods = [
        _VideosNamespace.wait_for_completion,
        _VideosNamespace.poll,
        _AsyncVideos.wait_for_completion,
        _AsyncVideos.poll,
        _Models3dNamespace.wait_for_completion,
        _AsyncModels3d.wait_for_completion,
    ]
    for fn in pollers + methods:
        default = inspect.signature(fn).parameters["timeout"].default
        assert default == JOB_DEFAULT_TIMEOUT, f"{fn.__qualname__}: {default}"
