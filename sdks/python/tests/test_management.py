"""Unit tests for the management facade (limits, end users, spend, gateway admin),
sync and async: each method sends the right method, path, query and body."""
from __future__ import annotations

import json
from typing import Any, Callable
from urllib.parse import parse_qsl, urlsplit

import httpx
import pytest
import respx

from litegen import AsyncLiteGenClient, LiteGenClient

BASE = "http://gw"
LIMITS = {"max_budget_usd": 10.0, "budget_duration": "30d"}
SCHEMA = {"id": "mock/x", "provider": "mock"}

# (call, method, path, query, body)
Case = tuple[Callable[[Any], Any], str, str, dict[str, str], Any]

CASES: list[Case] = [
    # keys
    (lambda c: c.keys.get("k1"), "GET", "/v1/keys/k1", {}, None),
    (lambda c: c.keys.block("k1"), "POST", "/v1/keys/k1/block", {}, None),
    (lambda c: c.keys.unblock("k/1"), "POST", "/v1/keys/k%2F1/unblock", {}, None),
    # limits
    (lambda c: c.limits.list(), "GET", "/v1/limits", {}, None),
    (lambda c: c.limits.list(entity_type="app", org_id="o2"), "GET", "/v1/limits", {"entity_type": "app", "org_id": "o2"}, None),
    (lambda c: c.limits.me(), "GET", "/v1/limits/me", {}, None),
    (lambda c: c.limits.get_key("k1"), "GET", "/v1/keys/k1/limits", {}, None),
    (lambda c: c.limits.set_key("k1", LIMITS), "PUT", "/v1/keys/k1/limits", {}, LIMITS),
    (lambda c: c.limits.delete_key("k1"), "DELETE", "/v1/keys/k1/limits", {}, None),
    (lambda c: c.limits.reset_key("k1"), "POST", "/v1/keys/k1/limits/reset", {}, None),
    (lambda c: c.limits.get_org("o1"), "GET", "/v1/orgs/o1/limits", {}, None),
    (lambda c: c.limits.set_org("o1", LIMITS), "PUT", "/v1/orgs/o1/limits", {}, LIMITS),
    (lambda c: c.limits.delete_org("o1"), "DELETE", "/v1/orgs/o1/limits", {}, None),
    (lambda c: c.limits.reset_org("o1"), "POST", "/v1/orgs/o1/limits/reset", {}, None),
    (lambda c: c.limits.get_app("a1"), "GET", "/v1/apps/a1/limits", {}, None),
    (lambda c: c.limits.set_app("a1", LIMITS), "PUT", "/v1/apps/a1/limits", {}, LIMITS),
    (lambda c: c.limits.delete_app("a1"), "DELETE", "/v1/apps/a1/limits", {}, None),
    (lambda c: c.limits.reset_app("a1"), "POST", "/v1/apps/a1/limits/reset", {}, None),
    (lambda c: c.limits.get_member("o1", "u1"), "GET", "/v1/orgs/o1/members/u1/limits", {}, None),
    (lambda c: c.limits.set_member("o1", "u1", LIMITS), "PUT", "/v1/orgs/o1/members/u1/limits", {}, LIMITS),
    (lambda c: c.limits.delete_member("o1", "u1"), "DELETE", "/v1/orgs/o1/members/u1/limits", {}, None),
    (lambda c: c.limits.reset_member("o1", "u1"), "POST", "/v1/orgs/o1/members/u1/limits/reset", {}, None),
    # end users
    (lambda c: c.end_users.list(), "GET", "/v1/end-users", {}, None),
    (lambda c: c.end_users.list(org_id="o9"), "GET", "/v1/end-users", {"org_id": "o9"}, None),
    (
        lambda c: c.end_users.create("c1", alias="Jane", limits=LIMITS),
        "POST", "/v1/end-users", {}, {"id": "c1", "alias": "Jane", "limits": LIMITS, "blocked": False},
    ),
    (lambda c: c.end_users.get("c 1"), "GET", "/v1/end-users/c%201", {}, None),
    (lambda c: c.end_users.update("c1", blocked=True), "PATCH", "/v1/end-users/c1", {}, {"blocked": True}),
    (lambda c: c.end_users.delete("c1", org_id="o2"), "DELETE", "/v1/end-users/c1", {"org_id": "o2"}, None),
    (lambda c: c.end_users.block("c1"), "POST", "/v1/end-users/c1/block", {}, None),
    (lambda c: c.end_users.unblock("c1"), "POST", "/v1/end-users/c1/unblock", {}, None),
    (lambda c: c.end_users.get_limits("c1"), "GET", "/v1/end-users/c1/limits", {}, None),
    (lambda c: c.end_users.set_limits("c1", LIMITS), "PUT", "/v1/end-users/c1/limits", {}, LIMITS),
    (lambda c: c.end_users.delete_limits("c1"), "DELETE", "/v1/end-users/c1/limits", {}, None),
    (lambda c: c.end_users.reset_limits("c1"), "POST", "/v1/end-users/c1/limits/reset", {}, None),
    # spend
    (lambda c: c.spend.report(), "GET", "/v1/spend/report", {"group_by": "day"}, None),
    (
        lambda c: c.spend.report(group_by="tag", from_="2026-10-01", to="2026-10-31", end_user="c1", key_id=None),
        "GET", "/v1/spend/report", {"group_by": "tag", "from": "2026-10-01", "to": "2026-10-31", "end_user": "c1"}, None,
    ),
    (
        lambda c: c.spend.logs(page=2, per_page=25, tag="x"),
        "GET", "/v1/spend/logs", {"page": "2", "per_page": "25", "tag": "x"}, None,
    ),
    # admin
    (lambda c: c.admin.models.list(), "GET", "/v1/admin/models", {}, None),
    (lambda c: c.admin.models.get("mock/x"), "GET", "/v1/admin/models/mock/x", {}, None),
    (lambda c: c.admin.models.create(SCHEMA), "POST", "/v1/admin/models", {}, SCHEMA),
    (lambda c: c.admin.models.replace("mock/x", SCHEMA), "PUT", "/v1/admin/models/mock/x", {}, SCHEMA),
    (lambda c: c.admin.models.set_disabled("mock/x", True), "PATCH", "/v1/admin/models/mock/x", {}, {"disabled": True}),
    (lambda c: c.admin.models.delete("mock/x"), "DELETE", "/v1/admin/models/mock/x", {}, None),
    (lambda c: c.admin.providers.list(), "GET", "/v1/admin/providers", {}, None),
    (lambda c: c.admin.providers.get("openai"), "GET", "/v1/admin/providers/openai", {}, None),
    (
        lambda c: c.admin.providers.configure("openai", api_key="sk", enabled=False),
        "PUT", "/v1/admin/providers/openai", {}, {"api_key": "sk", "enabled": False},
    ),
    (lambda c: c.admin.providers.reset("openai"), "DELETE", "/v1/admin/providers/openai", {}, None),
    (lambda c: c.admin.routes.get(), "GET", "/v1/admin/routes", {}, None),
    (
        lambda c: c.admin.routes.replace([{"model": "*", "deployments": [{"provider": "mock"}]}]),
        "PUT", "/v1/admin/routes", {}, {"routes": [{"model": "*", "deployments": [{"provider": "mock"}]}]},
    ),
    (lambda c: c.admin.routes.reset(), "DELETE", "/v1/admin/routes", {}, None),
    (lambda c: c.admin.settings.get(), "GET", "/v1/admin/settings", {}, None),
    (
        lambda c: c.admin.settings.update(cost_markup_percent=10.0),
        "PUT", "/v1/admin/settings", {}, {"cost_markup_percent": 10.0},
    ),
]

# Every response carries a `data` list so the unwrapping methods have one.
REPLY = {"data": [{"id": "x"}], "total": 0}


def _check(route: respx.Route, method: str, path: str, query: dict[str, str], body: Any) -> None:
    assert route.call_count == 1
    req: httpx.Request = route.calls.last.request
    url = urlsplit(str(req.url))
    assert req.method == method
    assert url.path == path
    assert dict(parse_qsl(url.query)) == query
    sent = json.loads(req.content) if req.content else None
    assert sent == body


@pytest.mark.parametrize("call,method,path,query,body", CASES, ids=[f"{c[1]} {c[2]} #{i}" for i, c in enumerate(CASES)])
def test_sync_management_calls(call: Callable[[Any], Any], method: str, path: str, query: dict[str, str], body: Any) -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        route = mock.route().mock(return_value=httpx.Response(200, json=REPLY))
        with LiteGenClient(api_key="k", base_url=BASE) as client:
            call(client)
        _check(route, method, path, query, body)


@pytest.mark.parametrize("call,method,path,query,body", CASES, ids=[f"{c[1]} {c[2]} #{i}" for i, c in enumerate(CASES)])
async def test_async_management_calls(call: Callable[[Any], Any], method: str, path: str, query: dict[str, str], body: Any) -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        route = mock.route().mock(return_value=httpx.Response(200, json=REPLY))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as client:
            await call(client)
        _check(route, method, path, query, body)


def test_list_methods_unwrap_data() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.route().mock(return_value=httpx.Response(200, json=REPLY))
        with LiteGenClient(api_key="k", base_url=BASE) as c:
            for fn in (c.limits.list, c.limits.me, c.end_users.list, c.admin.models.list, c.admin.providers.list):
                assert fn() == [{"id": "x"}]


async def test_async_list_methods_unwrap_data() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.route().mock(return_value=httpx.Response(200, json=REPLY))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            for fn in (c.limits.list, c.limits.me, c.end_users.list, c.admin.models.list, c.admin.providers.list):
                assert await fn() == [{"id": "x"}]
