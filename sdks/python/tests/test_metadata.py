"""``metadata`` (L17) and cancel's ``vendor_cancelled`` (L18), through both clients."""

from __future__ import annotations

import json

import pytest
import respx
from httpx import Response

from litegen import AsyncLiteGenClient, LiteGenClient

BASE = "http://localhost:4000"
PAGE = {"data": [], "total": 0, "page": 1, "per_page": 50, "total_pages": 0}


@pytest.fixture(autouse=True)
def _env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LITEGEN_API_KEY", "lg-test")
    monkeypatch.setenv("LITEGEN_BASE_URL", BASE)


@respx.mock
def test_logs_and_stats_filter_by_metadata() -> None:
    logs = respx.get(f"{BASE}/v1/logs").mock(return_value=Response(200, json=PAGE))
    stats = respx.get(f"{BASE}/v1/stats").mock(return_value=Response(200, json={}))
    client = LiteGenClient()
    client.logs.list(metadata={"project": "castle", "asset": "knight"})
    client.stats.get(metadata={"project": "castle"})
    client.stats.get()
    assert dict(logs.calls[0].request.url.params) == {
        "page": "1",
        "per_page": "50",
        "metadata.project": "castle",
        "metadata.asset": "knight",
    }
    assert dict(stats.calls[0].request.url.params) == {"metadata.project": "castle"}
    assert dict(stats.calls[1].request.url.params) == {}


@respx.mock
async def test_the_async_client_filters_the_same_way() -> None:
    logs = respx.get(f"{BASE}/v1/logs").mock(return_value=Response(200, json=PAGE))
    stats = respx.get(f"{BASE}/v1/stats").mock(return_value=Response(200, json={}))
    async with AsyncLiteGenClient() as client:
        await client.logs.list(metadata={"project": "castle"})
        await client.stats.get(metadata={"project": "castle"})
    assert logs.calls[0].request.url.params["metadata.project"] == "castle"
    assert stats.calls[0].request.url.params["metadata.project"] == "castle"


@respx.mock
def test_cancel_reports_whether_the_vendor_took_it() -> None:
    row = {"id": "litegen-vid-1", "status": "cancelled", "cost_usd": 0.0, "vendor_cancelled": False}
    cancel = respx.patch(f"{BASE}/v1/generations/litegen-vid-1").mock(return_value=Response(200, json=row))
    cancelled = LiteGenClient().generations.cancel("litegen-vid-1")
    assert (cancelled["vendor_cancelled"], cancelled["cost_usd"]) == (False, 0.0)
    assert json.loads(cancel.calls[0].request.content) == {"status": "cancelled"}
