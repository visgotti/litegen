"""Asynchronous LiteGen SDK client."""
from __future__ import annotations

import os
from typing import Any, AsyncIterator, Awaitable, Callable, Mapping

import httpx

from .client import _clean, _drop_none, _q, _raise_for_status
from .errors import LiteGenTimeoutError
from .polling import apoll_video, async_wait_for_video_completion


class AsyncLiteGenClient:
    """Asynchronous client for the LiteGen HTTP API.

    Example:
        >>> async with AsyncLiteGenClient(api_key="lg-...") as client:
        ...     img = await client.images.generate(prompt="...", model="...")
    """

    def __init__(
        self,
        *,
        api_key: str | None = None,
        base_url: str | None = None,
        timeout: float = 60.0,
        default_headers: Mapping[str, str] | None = None,
    ) -> None:
        self._api_key = api_key or os.environ.get("LITEGEN_API_KEY")
        self._base_url = (
            base_url or os.environ.get("LITEGEN_BASE_URL") or "http://localhost:4000"
        ).rstrip("/")
        self._timeout = timeout
        headers: dict[str, str] = {"Content-Type": "application/json"}
        if self._api_key:
            headers["Authorization"] = f"Bearer {self._api_key}"
        if default_headers:
            headers.update(default_headers)
        self._http = httpx.AsyncClient(base_url=self._base_url, headers=headers, timeout=timeout)

        self.images = _AsyncImages(self)
        self.videos = _AsyncVideos(self)
        self.models = _AsyncModels(self)
        self.health = _AsyncHealth(self)
        self.stats = _AsyncStats(self)
        self.logs = _AsyncLogs(self)
        self.keys = _AsyncKeys(self)
        self.cache = _AsyncCache(self)
        self.limits = _AsyncLimits(self)
        self.end_users = _AsyncEndUsers(self)
        self.spend = _AsyncSpend(self)
        self.admin = _AsyncAdmin(self)

    async def close(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncLiteGenClient":
        return self

    async def __aexit__(self, *_exc: object) -> None:
        await self.close()

    async def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        try:
            resp = await self._http.request(method, path, json=json, params=params)
        except httpx.TimeoutException as e:
            raise LiteGenTimeoutError(str(e)) from e
        if resp.status_code >= 400:
            _raise_for_status(resp)
        ctype = resp.headers.get("content-type", "")
        if ctype.startswith("application/json"):
            return resp.json()
        return resp.content


class _AsyncImages:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def generate(self, **req: Any) -> Any:
        return await self._c._request("POST", "/v1/images/generations", json=_clean(req))

    async def estimate_cost(self, **req: Any) -> Any:
        return await self._c._request("POST", "/v1/images/cost", json=_clean(req))


class AsyncVideoJob:
    """Awaitable, async-iterable handle returned by ``client.videos.generate``.

    Video generation is asynchronous on the server, so this handle mirrors the
    sync :class:`~litegen.client.VideoJob`::

        # await → the final completed / failed job (submits + polls under the hood)
        video = await client.videos.generate(prompt="...", model="...")
        print(video["video_url"])

        # …or stream progress (each update carries the unified 0-100 progress)
        async for update in client.videos.generate(prompt="...", model="..."):
            print(update["status"], update["progress"])

    There's no SSE on the wire. ``await job.submitted`` returns the initial
    submit response (id, status) without waiting. Submission is lazy and
    memoized: the POST fires on first use.
    """

    def __init__(
        self,
        submit: Callable[[], Awaitable[dict[str, Any]]],
        get_status: Callable[[str], Awaitable[dict[str, Any]]],
        *,
        interval: float = 2.0,
        timeout: float = 300.0,
    ) -> None:
        self._submit = submit
        self._get_status = get_status
        self._interval = interval
        self._timeout = timeout
        self._submitted: dict[str, Any] | None = None

    async def _ensure_submitted(self) -> dict[str, Any]:
        if self._submitted is None:
            self._submitted = await self._submit()
        return self._submitted

    @property
    def submitted(self) -> Awaitable[dict[str, Any]]:
        """Awaitable resolving to the initial submit response (id, status)."""
        return self._ensure_submitted()

    def __await__(self):
        return self._result().__await__()

    async def _result(self) -> dict[str, Any]:
        last = await self._ensure_submitted()
        async for update in self._stream():
            last = update
        return last

    def __aiter__(self) -> AsyncIterator[dict[str, Any]]:
        return self._stream()

    async def _stream(self) -> AsyncIterator[dict[str, Any]]:
        job = await self._ensure_submitted()
        async for update in apoll_video(
            job["id"], self._get_status, interval=self._interval, timeout=self._timeout
        ):
            yield update


class _AsyncVideos:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    def generate(self, **req: Any) -> AsyncVideoJob:
        """Submit a video job, returning an awaitable, async-iterable handle.

        ``await`` it for the finished job, or ``async for`` over it to stream
        progress. ``interval`` / ``timeout`` (seconds) tune the polling cadence
        and are not sent as part of the request body.
        """
        interval = req.pop("interval", 2.0)
        timeout = req.pop("timeout", 300.0)
        body = _clean(req)

        async def _submit() -> dict[str, Any]:
            return await self._c._request("POST", "/v1/videos/generations", json=body)

        return AsyncVideoJob(
            _submit, self.get_status, interval=interval, timeout=timeout
        )

    async def estimate_cost(self, **req: Any) -> Any:
        return await self._c._request("POST", "/v1/videos/cost", json=_clean(req))

    async def get_status(self, video_id: str) -> Any:
        return await self._c._request("GET", f"/v1/videos/{video_id}")

    async def wait_for_completion(
        self, video_id: str, *, interval: float = 2.0, timeout: float = 300.0
    ) -> Any:
        return await async_wait_for_video_completion(
            video_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self, video_id: str, *, interval: float = 2.0, timeout: float = 300.0
    ) -> AsyncIterator[Any]:
        """Stream status updates as an async generator::

            async for update in client.videos.poll(job["id"]):
                print(update["status"], update["progress"])
        """
        return apoll_video(
            video_id, self.get_status, interval=interval, timeout=timeout
        )


class _AsyncModels:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def list(self) -> Any:
        resp = await self._c._request("GET", "/v1/models")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    async def get(self, model_id: str) -> Any:
        return await self._c._request("GET", f"/v1/models/{model_id}")


class _AsyncHealth:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def check(self) -> Any:
        return await self._c._request("GET", "/health")

    async def live(self) -> Any:
        return await self._c._request("GET", "/health/live")


class _AsyncStats:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def get(self) -> Any:
        return await self._c._request("GET", "/v1/stats")


class _AsyncLogs:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def list(self, *, page: int = 1, per_page: int = 50) -> Any:
        return await self._c._request(
            "GET", "/v1/logs", params={"page": page, "per_page": per_page}
        )


class _AsyncKeys:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def create(self, name: str) -> Any:
        return await self._c._request("POST", "/v1/keys", json={"name": name})

    async def list(self) -> Any:
        resp = await self._c._request("GET", "/v1/keys")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    async def revoke(self, key_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/keys/{key_id}")

    async def get(self, key_id: str) -> Any:
        return await self._c._request("GET", f"/v1/keys/{_q(key_id)}")

    async def block(self, key_id: str) -> Any:
        """Refuse every request made with this key until unblocked."""
        return await self._c._request("POST", f"/v1/keys/{_q(key_id)}/block")

    async def unblock(self, key_id: str) -> Any:
        return await self._c._request("POST", f"/v1/keys/{_q(key_id)}/unblock")


class _AsyncCache:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def clear(self) -> Any:
        return await self._c._request("DELETE", "/v1/cache")


# ─── Management: limits, end users, spend, gateway admin ─────────────────────


class _AsyncLimits:
    """Budgets and limits (``/v1/limits``, ``…/limits`` on keys, orgs, apps, members).

    A limits body (``LimitsInput``) takes ``max_budget_usd``, ``soft_budget_usd``,
    ``budget_duration`` (``30s``/``30m``/``12h``/``30d``/``1w``/``1mo``),
    ``allowed_models``, ``max_parallel_requests``, ``rpm_limit`` and ``metadata``.
    ``set_*`` replaces every field; ``reset_*`` zeroes the current window's spend.
    """

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def list(self, *, entity_type: str | None = None, org_id: str | None = None) -> list[Any]:
        resp = await self._c._request(
            "GET", "/v1/limits", params=_drop_none({"entity_type": entity_type, "org_id": org_id})
        )
        return resp["data"]

    async def me(self) -> Any:
        """The limits that apply to the caller: its key, org, app and member rows."""
        return (await self._c._request("GET", "/v1/limits/me"))["data"]

    async def get_key(self, key_id: str) -> Any:
        return await self._c._request("GET", f"/v1/keys/{_q(key_id)}/limits")

    async def set_key(self, key_id: str, limits: Mapping[str, Any]) -> Any:
        return await self._c._request("PUT", f"/v1/keys/{_q(key_id)}/limits", json=dict(limits))

    async def delete_key(self, key_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/keys/{_q(key_id)}/limits")

    async def reset_key(self, key_id: str) -> Any:
        return await self._c._request("POST", f"/v1/keys/{_q(key_id)}/limits/reset")

    async def get_org(self, org_id: str) -> Any:
        return await self._c._request("GET", f"/v1/orgs/{_q(org_id)}/limits")

    async def set_org(self, org_id: str, limits: Mapping[str, Any]) -> Any:
        return await self._c._request("PUT", f"/v1/orgs/{_q(org_id)}/limits", json=dict(limits))

    async def delete_org(self, org_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/orgs/{_q(org_id)}/limits")

    async def reset_org(self, org_id: str) -> Any:
        return await self._c._request("POST", f"/v1/orgs/{_q(org_id)}/limits/reset")

    async def get_app(self, app_id: str) -> Any:
        return await self._c._request("GET", f"/v1/apps/{_q(app_id)}/limits")

    async def set_app(self, app_id: str, limits: Mapping[str, Any]) -> Any:
        return await self._c._request("PUT", f"/v1/apps/{_q(app_id)}/limits", json=dict(limits))

    async def delete_app(self, app_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/apps/{_q(app_id)}/limits")

    async def reset_app(self, app_id: str) -> Any:
        return await self._c._request("POST", f"/v1/apps/{_q(app_id)}/limits/reset")

    async def get_member(self, org_id: str, user_id: str) -> Any:
        return await self._c._request("GET", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits")

    async def set_member(self, org_id: str, user_id: str, limits: Mapping[str, Any]) -> Any:
        return await self._c._request(
            "PUT", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits", json=dict(limits)
        )

    async def delete_member(self, org_id: str, user_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits")

    async def reset_member(self, org_id: str, user_id: str) -> Any:
        return await self._c._request("POST", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits/reset")


class _AsyncEndUsers:
    """End users (your callers' customers), named per request with ``x-litegen-end-user``.

    ``org_id`` is for the platform admin only, to act on another org.
    """

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def list(self, *, org_id: str | None = None) -> list[Any]:
        return (await self._c._request("GET", "/v1/end-users", params=_drop_none({"org_id": org_id})))["data"]

    async def create(
        self,
        end_user_id: str,
        *,
        alias: str | None = None,
        blocked: bool = False,
        metadata: Mapping[str, Any] | None = None,
        limits: Mapping[str, Any] | None = None,
        org_id: str | None = None,
    ) -> Any:
        body = _drop_none({"id": end_user_id, "alias": alias, "metadata": metadata, "limits": limits})
        body["blocked"] = blocked
        return await self._c._request(
            "POST", "/v1/end-users", json=body, params=_drop_none({"org_id": org_id})
        )

    async def get(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "GET", f"/v1/end-users/{_q(end_user_id)}", params=_drop_none({"org_id": org_id})
        )

    async def update(
        self,
        end_user_id: str,
        *,
        alias: str | None = None,
        blocked: bool | None = None,
        metadata: Mapping[str, Any] | None = None,
        org_id: str | None = None,
    ) -> Any:
        body = _drop_none({"alias": alias, "blocked": blocked, "metadata": metadata})
        return await self._c._request(
            "PATCH", f"/v1/end-users/{_q(end_user_id)}", json=body, params=_drop_none({"org_id": org_id})
        )

    async def delete(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "DELETE", f"/v1/end-users/{_q(end_user_id)}", params=_drop_none({"org_id": org_id})
        )

    async def block(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/block", params=_drop_none({"org_id": org_id})
        )

    async def unblock(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/unblock", params=_drop_none({"org_id": org_id})
        )

    async def get_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "GET", f"/v1/end-users/{_q(end_user_id)}/limits", params=_drop_none({"org_id": org_id})
        )

    async def set_limits(self, end_user_id: str, limits: Mapping[str, Any], *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "PUT",
            f"/v1/end-users/{_q(end_user_id)}/limits",
            json=dict(limits),
            params=_drop_none({"org_id": org_id}),
        )

    async def delete_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "DELETE", f"/v1/end-users/{_q(end_user_id)}/limits", params=_drop_none({"org_id": org_id})
        )

    async def reset_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return await self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/limits/reset", params=_drop_none({"org_id": org_id})
        )


class _AsyncSpend:
    """Spend reports (``/v1/spend/report``) and the attributed request log (``/v1/spend/logs``).

    Filters: ``from_``/``to`` (RFC 3339 or ``YYYY-MM-DD``), ``org_id`` (platform
    admin only), ``app_id``, ``key_id``, ``user_id``, ``end_user``, ``model``,
    ``provider``, ``tag``.
    """

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    @staticmethod
    def _filters(from_: str | None, filters: Mapping[str, Any]) -> dict[str, Any]:
        return _drop_none({"from": from_, **filters})

    async def report(self, *, group_by: str = "day", from_: str | None = None, **filters: Any) -> Any:
        params = self._filters(from_, filters)
        params["group_by"] = group_by
        return await self._c._request("GET", "/v1/spend/report", params=params)

    async def logs(self, *, page: int = 1, per_page: int = 50, from_: str | None = None, **filters: Any) -> Any:
        params = self._filters(from_, filters)
        params.update(page=page, per_page=per_page)
        return await self._c._request("GET", "/v1/spend/logs", params=params)


class _AsyncAdminModels:
    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def list(self) -> list[Any]:
        """Every model, catalog and runtime, enabled or not."""
        return (await self._c._request("GET", "/v1/admin/models"))["data"]

    async def get(self, model_id: str) -> Any:
        return await self._c._request("GET", f"/v1/admin/models/{model_id}")

    async def create(self, schema: Mapping[str, Any]) -> Any:
        """Add a model at runtime; ``schema`` is a ``models/*.yaml`` row as JSON."""
        return await self._c._request("POST", "/v1/admin/models", json=dict(schema))

    async def replace(self, model_id: str, schema: Mapping[str, Any]) -> Any:
        return await self._c._request("PUT", f"/v1/admin/models/{model_id}", json=dict(schema))

    async def set_disabled(self, model_id: str, disabled: bool) -> Any:
        return await self._c._request("PATCH", f"/v1/admin/models/{model_id}", json={"disabled": disabled})

    async def delete(self, model_id: str) -> Any:
        return await self._c._request("DELETE", f"/v1/admin/models/{model_id}")


class _AsyncAdminProviders:
    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def list(self) -> list[Any]:
        """Every provider adapter and its effective config; secrets are masked."""
        return (await self._c._request("GET", "/v1/admin/providers"))["data"]

    async def get(self, name: str) -> Any:
        return await self._c._request("GET", f"/v1/admin/providers/{_q(name)}")

    async def configure(self, name: str, **config: Any) -> Any:
        """Set ``api_key``, ``api_keys``, ``key_id``, ``key_secret``, ``api_base``,
        ``model_mapping``, ``extra_headers``, ``options``, ``region`` or ``enabled``."""
        return await self._c._request("PUT", f"/v1/admin/providers/{_q(name)}", json=config)

    async def reset(self, name: str) -> Any:
        """Drop the runtime config; the provider falls back to its env/YAML config."""
        return await self._c._request("DELETE", f"/v1/admin/providers/{_q(name)}")


class _AsyncAdminRoutes:
    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def get(self) -> Any:
        return await self._c._request("GET", "/v1/admin/routes")

    async def replace(self, routes: list[Mapping[str, Any]]) -> Any:
        return await self._c._request("PUT", "/v1/admin/routes", json={"routes": [dict(r) for r in routes]})

    async def reset(self) -> Any:
        return await self._c._request("DELETE", "/v1/admin/routes")


class _AsyncAdminSettings:
    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def get(self) -> Any:
        return await self._c._request("GET", "/v1/admin/settings")

    async def update(self, **settings: Any) -> Any:
        """Replace the gateway settings; an omitted field is cleared."""
        return await self._c._request("PUT", "/v1/admin/settings", json=settings)


class _AsyncAdmin:
    """Gateway admin: runtime models, provider configs, routes and settings."""

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self.models = _AsyncAdminModels(client)
        self.providers = _AsyncAdminProviders(client)
        self.routes = _AsyncAdminRoutes(client)
        self.settings = _AsyncAdminSettings(client)
