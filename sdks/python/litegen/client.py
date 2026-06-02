"""Synchronous LiteGen SDK client."""
from __future__ import annotations

import os
from typing import Any, Callable, Iterator, Mapping

import httpx

from .errors import (
    LiteGenAPIError,
    LiteGenTimeoutError,
    LiteGenValidationError,
)
from .polling import poll_video, wait_for_video_completion


class LiteGenClient:
    """Synchronous client for the LiteGen HTTP API.

    Example:
        >>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
        >>> img = client.images.generate(prompt="a cat", model="openai/dall-e-3")
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
        self._http = httpx.Client(base_url=self._base_url, headers=headers, timeout=timeout)

        self.images = _ImagesNamespace(self)
        self.videos = _VideosNamespace(self)
        self.models = _ModelsNamespace(self)
        self.health = _HealthNamespace(self)
        self.stats = _StatsNamespace(self)
        self.logs = _LogsNamespace(self)
        self.keys = _KeysNamespace(self)
        self.cache = _CacheNamespace(self)

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "LiteGenClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        try:
            resp = self._http.request(method, path, json=json, params=params)
        except httpx.TimeoutException as e:
            raise LiteGenTimeoutError(str(e)) from e
        if resp.status_code >= 400:
            _raise_for_status(resp)
        ctype = resp.headers.get("content-type", "")
        if ctype.startswith("application/json"):
            return resp.json()
        return resp.content


def _raise_for_status(resp: httpx.Response) -> None:
    detail: dict[str, Any] = {}
    try:
        payload = resp.json()
        if isinstance(payload, dict):
            detail = payload.get("error") or {}
    except Exception:
        detail = {}
    message = detail.get("message") or resp.text or f"HTTP {resp.status_code}"
    err_type = detail.get("type") or "api_error"
    code = detail.get("code")
    provider_error = detail.get("provider_error")
    cls = LiteGenValidationError if err_type == "validation_error" else LiteGenAPIError
    raise cls(
        resp.status_code,
        message,
        type=err_type,
        code=str(code) if code is not None else None,
        provider_error=provider_error,
    )


def _clean(req: dict[str, Any]) -> dict[str, Any]:
    """Drop None values so server defaults are honored."""
    return {k: v for k, v in req.items() if v is not None}


class _ImagesNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/images/generations", json=_clean(req))

    def estimate_cost(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/images/cost", json=_clean(req))


class VideoJob:
    """Handle returned by :meth:`_VideosNamespace.generate`.

    Video generation is asynchronous on the server, so this handle lets you
    either block for the finished job or stream progress as it runs::

        # block until the video is done (or failed) — returns the final job
        video = client.videos.generate(prompt="...", model="...").result()
        print(video["video_url"])

        # …or stream progress (each update carries the unified 0-100 progress)
        for update in client.videos.generate(prompt="...", model="..."):
            print(update["status"], update["progress"])

    There's no SSE on the wire — it submits then polls under the hood.
    ``job.submitted`` returns the initial submit response (id, status) without
    waiting. Submission is lazy and memoized: the POST fires on first use.
    """

    def __init__(
        self,
        submit: Callable[[], dict[str, Any]],
        get_status: Callable[[str], dict[str, Any]],
        *,
        interval: float = 2.0,
        timeout: float = 300.0,
    ) -> None:
        self._submit = submit
        self._get_status = get_status
        self._interval = interval
        self._timeout = timeout
        self._submitted: dict[str, Any] | None = None

    @property
    def submitted(self) -> dict[str, Any]:
        """The initial submit response (id, status) — does not wait for completion."""
        if self._submitted is None:
            self._submitted = self._submit()
        return self._submitted

    def __iter__(self) -> Iterator[dict[str, Any]]:
        return poll_video(
            self.submitted["id"],
            self._get_status,
            interval=self._interval,
            timeout=self._timeout,
        )

    def result(self) -> dict[str, Any]:
        """Block until the job reaches a terminal status; return the final job."""
        last: dict[str, Any] = self.submitted
        for last in self:
            pass
        return last


class _VideosNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(self, **req: Any) -> VideoJob:
        """Submit a video job, returning a :class:`VideoJob` handle.

        Call ``.result()`` to block until the video is finished, or iterate the
        handle to stream progress. ``interval`` / ``timeout`` (seconds) tune the
        polling cadence and are not sent as part of the request body.
        """
        interval = req.pop("interval", 2.0)
        timeout = req.pop("timeout", 300.0)
        body = _clean(req)
        return VideoJob(
            lambda: self._c._request("POST", "/v1/videos/generations", json=body),
            self.get_status,
            interval=interval,
            timeout=timeout,
        )

    def estimate_cost(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/videos/cost", json=_clean(req))

    def get_status(self, video_id: str) -> Any:
        return self._c._request("GET", f"/v1/videos/{video_id}")

    def wait_for_completion(
        self,
        video_id: str,
        *,
        interval: float = 2.0,
        timeout: float = 300.0,
    ) -> Any:
        return wait_for_video_completion(
            video_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self,
        video_id: str,
        *,
        interval: float = 2.0,
        timeout: float = 300.0,
    ) -> Iterator[Any]:
        """Stream status updates as a generator::

            for update in client.videos.poll(job["id"]):
                print(update["status"], update["progress"])

        ``progress`` is the unified 0-100 value (providers without fine-grained
        reporting step toward 100). Yields the terminal update last.
        """
        return poll_video(
            video_id, self.get_status, interval=interval, timeout=timeout
        )


class _ModelsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self) -> list[Any]:
        resp = self._c._request("GET", "/v1/models")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    def get(self, model_id: str) -> Any:
        return self._c._request("GET", f"/v1/models/{model_id}")


class _HealthNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def check(self) -> Any:
        return self._c._request("GET", "/health")

    def live(self) -> Any:
        return self._c._request("GET", "/health/live")


class _StatsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get(self) -> Any:
        return self._c._request("GET", "/v1/stats")


class _LogsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, page: int = 1, per_page: int = 50) -> Any:
        return self._c._request("GET", "/v1/logs", params={"page": page, "per_page": per_page})


class _KeysNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def create(self, name: str) -> Any:
        return self._c._request("POST", "/v1/keys", json={"name": name})

    def list(self) -> list[Any]:
        resp = self._c._request("GET", "/v1/keys")
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    def revoke(self, key_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/keys/{key_id}")


class _CacheNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def clear(self) -> Any:
        return self._c._request("DELETE", "/v1/cache")
