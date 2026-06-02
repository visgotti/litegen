"""Asynchronous LiteGen SDK client."""
from __future__ import annotations

import os
from typing import Any, AsyncIterator, Awaitable, Callable, Mapping

import httpx

from .client import _clean, _raise_for_status
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


class _AsyncCache:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def clear(self) -> Any:
        return await self._c._request("DELETE", "/v1/cache")
