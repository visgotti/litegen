"""Unit tests for the management facade (limits, end users, spend, gateway admin),
sync and async: each method sends the right method, path, query and body."""
from __future__ import annotations

import json
from typing import Any, Callable
from urllib.parse import parse_qsl, urlsplit

import httpx
import pytest
import respx

from litegen import AsyncLiteGenClient, LiteGenAPIError, LiteGenClient

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
    # The key PATCH is a JSON merge patch: what is given is sent, None as null
    # (which clears the field), and what is left out is not sent (kept).
    (lambda c: c.keys.patch("k/1", name="ci"), "PATCH", "/v1/keys/k%2F1", {}, {"name": "ci"}),
    (
        lambda c: c.keys.patch("k1", token_quota=None, rpm_limit=None, webhook_url=None, expires_at=None),
        "PATCH", "/v1/keys/k1", {}, {"token_quota": None, "rpm_limit": None, "webhook_url": None, "expires_at": None},
    ),
    (
        lambda c: c.keys.patch("k1", webhook_url="", rpm_limit=0, is_active=False),
        "PATCH", "/v1/keys/k1", {}, {"webhook_url": "", "rpm_limit": 0, "is_active": False},
    ),
    # a key's webhook delivery attempts
    (
        lambda c: c.keys.list_webhook_deliveries("k/1"),
        "GET", "/v1/keys/k%2F1/webhook-deliveries", {"page": "1", "per_page": "50"}, None,
    ),
    (
        lambda c: c.keys.list_webhook_deliveries("k1", page=3, per_page=5),
        "GET", "/v1/keys/k1/webhook-deliveries", {"page": "3", "per_page": "5"}, None,
    ),
    (
        lambda c: c.keys.redeliver_webhook_delivery("k/1", "whd-1"),
        "POST", "/v1/keys/k%2F1/webhook-deliveries/whd-1/redeliver", {}, None,
    ),
    # the audit log; org_id is the master key's, to read another org's entries
    (lambda c: c.audit.list(), "GET", "/v1/audit", {"page": "1", "per_page": "50"}, None),
    (
        lambda c: c.audit.list(
            org_id="o2", actor_key_id="k1", action="key.create", from_="2026-11-01", to="2026-11-15", page=2, per_page=10
        ),
        "GET", "/v1/audit",
        {
            "org_id": "o2", "actor_key_id": "k1", "action": "key.create", "from": "2026-11-01", "to": "2026-11-15",
            "page": "2", "per_page": "10",
        },
        None,
    ),
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
    # who may open an app's stored files
    (lambda c: c.file_access.get_app("a/1"), "GET", "/v1/apps/a%2F1/access", {}, None),
    (
        lambda c: c.file_access.set_app("a1", mode="signed", signed_url_ttl_seconds=900),
        "PUT", "/v1/apps/a1/access", {}, {"mode": "signed", "signed_url_ttl_seconds": 900},
    ),
    (lambda c: c.file_access.set_app("a1"), "PUT", "/v1/apps/a1/access", {}, {}),
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
    # PATCH changes what it is given and keeps the rest; None (null) clears a field.
    (
        lambda c: c.admin.providers.patch("openai", api_base=None, enabled=True),
        "PATCH", "/v1/admin/providers/openai", {}, {"api_base": None, "enabled": True},
    ),
    (
        lambda c: c.admin.providers.patch("open/ai", model_mapping={"a": "b"}, options=None),
        "PATCH", "/v1/admin/providers/open%2Fai", {}, {"model_mapping": {"a": "b"}, "options": None},
    ),
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
    # an account's password reset link
    (lambda c: c.users.password_reset_link("u/1"), "POST", "/v1/users/u%2F1/password-reset", {}, None),
    # an org's invitations
    (
        lambda c: c.orgs.members.invite("o/1", email="ann@example.com", role="member"),
        "POST", "/v1/orgs/o%2F1/members", {}, {"email": "ann@example.com", "role": "member"},
    ),
    (lambda c: c.orgs.invitations.list("o/1"), "GET", "/v1/orgs/o%2F1/invitations", {}, None),
    (lambda c: c.orgs.invitations.revoke("o1", "inv/1"), "DELETE", "/v1/orgs/o1/invitations/inv%2F1", {}, None),
    # oauth: the MCP consent page's API
    (lambda c: c.oauth.get_request("r/1"), "GET", "/v1/oauth/requests/r%2F1", {}, None),
    (
        lambda c: c.oauth.approve_request("r1", org_id="o1", scopes=["ai:read"]),
        "POST", "/v1/oauth/requests/r1/approve", {}, {"org_id": "o1", "scopes": ["ai:read"]},
    ),
    (
        lambda c: c.oauth.approve_request("r1", org_id="o1", app_id="a1", scopes=["ai:generate", "ai:read"]),
        "POST", "/v1/oauth/requests/r1/approve", {}, {"org_id": "o1", "app_id": "a1", "scopes": ["ai:generate", "ai:read"]},
    ),
    (lambda c: c.oauth.deny_request("r1"), "POST", "/v1/oauth/requests/r1/deny", {}, None),
    (lambda c: c.oauth.list_grants(), "GET", "/v1/oauth/grants", {}, None),
    (lambda c: c.oauth.list_grants(user_id="u1"), "GET", "/v1/oauth/grants", {"user_id": "u1"}, None),
    (lambda c: c.oauth.revoke_grant("g/1"), "DELETE", "/v1/oauth/grants/g%2F1", {}, None),
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


INVITE = {
    "id": "inv1",
    "email": "ann@example.com",
    "role": "member",
    "org_id": "o1",
    "expires_at": "2026-11-22T00:00:00Z",
    "token": "tok-1",
    "invite_url": None,
}
PENDING = [
    {
        "id": "inv1",
        "email": "ann@example.com",
        "role": "member",
        "org_id": "o1",
        "invited_by": None,
        "created_at": "2026-11-15T00:00:00Z",
        "expires_at": "2026-11-22T00:00:00Z",
    }
]


def _invitation_routes(mock: respx.MockRouter) -> None:
    mock.post("/v1/orgs/o1/members").mock(return_value=httpx.Response(200, json=INVITE))
    mock.get("/v1/orgs/o1/invitations").mock(return_value=httpx.Response(200, json=PENDING))
    mock.delete("/v1/orgs/o1/invitations/inv1").mock(return_value=httpx.Response(204))


def test_invitations_answer_as_the_server_sends_and_a_revoke_answers_nothing() -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        _invitation_routes(mock)
        with LiteGenClient(api_key="k", base_url=BASE) as c:
            assert c.orgs.members.invite("o1", email="ann@example.com", role="member") == INVITE
            assert c.orgs.invitations.list("o1") == PENDING
            assert c.orgs.invitations.revoke("o1", "inv1") is None


async def test_async_invitations_answer_as_the_server_sends_and_a_revoke_answers_nothing() -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        _invitation_routes(mock)
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            assert await c.orgs.members.invite("o1", email="ann@example.com", role="member") == INVITE
            assert await c.orgs.invitations.list("o1") == PENDING
            assert await c.orgs.invitations.revoke("o1", "inv1") is None


GONE = {"error": {"message": "No unaccepted invitation with this id in this org", "type": "api_error", "code": "invitation_not_found"}}


def test_revoking_an_invitation_that_is_gone_raises_its_code() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.delete("/v1/orgs/o1/invitations/inv1").mock(return_value=httpx.Response(404, json=GONE))
        with LiteGenClient(api_key="k", base_url=BASE) as c, pytest.raises(LiteGenAPIError) as exc:
            c.orgs.invitations.revoke("o1", "inv1")
    assert (exc.value.status, exc.value.code) == (404, "invitation_not_found")


async def test_async_revoking_an_invitation_that_is_gone_raises_its_code() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.delete("/v1/orgs/o1/invitations/inv1").mock(return_value=httpx.Response(404, json=GONE))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            with pytest.raises(LiteGenAPIError) as exc:
                await c.orgs.invitations.revoke("o1", "inv1")
    assert (exc.value.status, exc.value.code) == (404, "invitation_not_found")


# An entry filed under no org has no `org_id`; every attempt of one delivery
# carries its `delivery_id`.
AUDIT_PAGE = {
    "data": [
        {
            "id": "a1",
            "actor_label": "master-key",
            "action": "settings.update",
            "target_type": "settings",
            "target_id": "gateway",
            "created_at": "2026-11-15T00:00:00Z",
        }
    ],
    "total": 1,
    "page": 1,
    "per_page": 50,
    "total_pages": 1,
}
DELIVERY_PAGE = {
    "data": [
        {
            "id": "d2",
            "key_id": "k1",
            "generation_id": "g1",
            "url": "https://hooks.example.com/litegen",
            "attempt_number": 2,
            "success": True,
            "status_code": 200,
            "payload_json": "{}",
            "delivery_id": "whd-0b7e1d3c-5a2f-4e8b-9c61-2f4d8a7e3b10",
            "created_at": "2026-11-15T00:00:30Z",
        }
    ],
    "total": 2,
    "page": 1,
    "per_page": 1,
    "total_pages": 2,
}


def _page_routes(mock: respx.MockRouter) -> None:
    mock.get("/v1/audit").mock(return_value=httpx.Response(200, json=AUDIT_PAGE))
    mock.get("/v1/keys/k1/webhook-deliveries").mock(return_value=httpx.Response(200, json=DELIVERY_PAGE))


def test_the_audit_and_webhook_delivery_pages_come_back_whole() -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        _page_routes(mock)
        with LiteGenClient(api_key="k", base_url=BASE) as c:
            assert c.audit.list() == AUDIT_PAGE
            assert c.keys.list_webhook_deliveries("k1", per_page=1) == DELIVERY_PAGE


async def test_async_audit_and_webhook_delivery_pages_come_back_whole() -> None:
    with respx.mock(base_url=BASE, assert_all_called=True) as mock:
        _page_routes(mock)
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            assert await c.audit.list() == AUDIT_PAGE
            assert await c.keys.list_webhook_deliveries("k1", per_page=1) == DELIVERY_PAGE


REDELIVER = "/v1/keys/k1/webhook-deliveries/whd-1/redeliver"
STATE = {
    "delivery_id": "whd-1",
    "generation_id": "g1",
    "status": "pending",
    "attempts": 0,
    "next_attempt_at": "2026-11-15T11:40:00Z",
    "last_status_code": 500,
    "last_error": "the endpoint answered 500",
    "created_at": "2026-11-14T09:00:00Z",
    "finished_at": None,
    "redelivered_at": "2026-11-15T11:40:00Z",
}


def test_a_redeliver_answers_the_deliverys_state() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(REDELIVER).mock(return_value=httpx.Response(202, json=STATE))
        with LiteGenClient(api_key="k", base_url=BASE) as c:
            assert c.keys.redeliver_webhook_delivery("k1", "whd-1") == STATE


async def test_async_redeliver_answers_the_deliverys_state() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(REDELIVER).mock(return_value=httpx.Response(202, json=STATE))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            assert await c.keys.redeliver_webhook_delivery("k1", "whd-1") == STATE


# A delivery still being sent is a 409; a second redeliver within a minute a 429
# whose Retry-After says when to ask again.
REFUSALS = [
    (409, "delivery_pending", "invalid_request_error", {}, None),
    (429, "redeliver_too_soon", "rate_limit_error", {"retry-after": "42"}, 42.0),
]


def _refusal(status: int, code: str, kind: str, headers: dict[str, str]) -> httpx.Response:
    return httpx.Response(status, json={"error": {"code": code, "message": code, "type": kind}}, headers=headers)


@pytest.mark.parametrize("status,code,kind,headers,retry_after", REFUSALS)
def test_a_refused_redeliver_raises_its_code(
    status: int, code: str, kind: str, headers: dict[str, str], retry_after: float | None
) -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(REDELIVER).mock(return_value=_refusal(status, code, kind, headers))
        with LiteGenClient(api_key="k", base_url=BASE) as c, pytest.raises(LiteGenAPIError) as exc:
            c.keys.redeliver_webhook_delivery("k1", "whd-1")
    assert (exc.value.status, exc.value.code, exc.value.retry_after) == (status, code, retry_after)


@pytest.mark.parametrize("status,code,kind,headers,retry_after", REFUSALS)
async def test_async_refused_redeliver_raises_its_code(
    status: int, code: str, kind: str, headers: dict[str, str], retry_after: float | None
) -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(REDELIVER).mock(return_value=_refusal(status, code, kind, headers))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            with pytest.raises(LiteGenAPIError) as exc:
                await c.keys.redeliver_webhook_delivery("k1", "whd-1")
    assert (exc.value.status, exc.value.code, exc.value.retry_after) == (status, code, retry_after)


RESET = "/v1/users/u1/password-reset"
LINK = {
    "user_id": "u1",
    "email": "ann@example.com",
    "token": "tok-reset",
    "reset_url": None,
    "expires_at": "2026-11-15T13:00:00Z",
}


def test_a_password_reset_link_comes_back_as_sent() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(RESET).mock(return_value=httpx.Response(200, json=LINK))
        with LiteGenClient(api_key="k", base_url=BASE) as c:
            assert c.users.password_reset_link("u1") == LINK


async def test_async_password_reset_link_comes_back_as_sent() -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(RESET).mock(return_value=httpx.Response(200, json=LINK))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            assert await c.users.password_reset_link("u1") == LINK


RESET_REFUSALS = [(403, "forbidden_permission"), (404, "user_not_found")]


@pytest.mark.parametrize("status,code", RESET_REFUSALS)
def test_a_refused_password_reset_link_raises_its_code(status: int, code: str) -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(RESET).mock(return_value=_refusal(status, code, "api_error", {}))
        with LiteGenClient(api_key="k", base_url=BASE) as c, pytest.raises(LiteGenAPIError) as exc:
            c.users.password_reset_link("u1")
    assert (exc.value.status, exc.value.code) == (status, code)


@pytest.mark.parametrize("status,code", RESET_REFUSALS)
async def test_async_refused_password_reset_link_raises_its_code(status: int, code: str) -> None:
    with respx.mock(base_url=BASE) as mock:
        mock.post(RESET).mock(return_value=_refusal(status, code, "api_error", {}))
        async with AsyncLiteGenClient(api_key="k", base_url=BASE) as c:
            with pytest.raises(LiteGenAPIError) as exc:
                await c.users.password_reset_link("u1")
    assert (exc.value.status, exc.value.code) == (status, code)
