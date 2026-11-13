"""Asynchronous LiteGen SDK client."""
from __future__ import annotations

import asyncio
import os
from typing import Any, AsyncIterator, Awaitable, Callable, Generator, Mapping
from urllib.parse import quote

import httpx
from typing_extensions import Unpack

from .client import (
    DEFAULT_TIMEOUT,
    _DROPPED_PARAMS_HEADER,
    _OCTET_STREAM,
    _clean,
    _conversion,
    _convert_params,
    _idempotency,
    _prefer_async,
    _drop_none,
    _metadata_params,
    _parse_dropped_params,
    _q,
    _raise_for_status,
)
from .errors import LiteGenTimeoutError
from .polling import (
    JOB_DEFAULT_TIMEOUT,
    MODEL3D_DEFAULT_TIMEOUT,
    apoll_model3d,
    apoll_video,
    async_wait_for_job,
    async_wait_for_model3d_completion,
    async_wait_for_video_completion,
)
from .types import (
    CancelledGeneration,
    CostEstimate,
    Generation,
    GenerationAssetList,
    GenerationPage,
    ImageSegmentationResponse,
    MediaType,
    Model3dConversion,
    Model3dConvertFormats,
    Model3dFormat,
    Model3dGenerateParams,
    Model3dGenerationResponse,
)


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
        timeout: float = DEFAULT_TIMEOUT,
        default_headers: Mapping[str, str] | None = None,
        on_response: Callable[[httpx.Response], None] | None = None,
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
        # ``on_response`` sees every response, errors included, before it is
        # read: for the headers no method returns (``response_meta``).
        hooks = None
        if on_response:

            async def _seen(resp: httpx.Response) -> None:
                on_response(resp)

            hooks = {"response": [_seen]}
        self._http = httpx.AsyncClient(base_url=self._base_url, headers=headers, timeout=timeout, event_hooks=hooks)

        self.images = _AsyncImages(self)
        self.videos = _AsyncVideos(self)
        self.models3d = _AsyncModels3d(self)
        self.generations = _AsyncGenerations(self)
        self.models = _AsyncModels(self)
        self.health = _AsyncHealth(self)
        self.stats = _AsyncStats(self)
        self.logs = _AsyncLogs(self)
        self.keys = _AsyncKeys(self)
        self.cache = _AsyncCache(self)
        self.limits = _AsyncLimits(self)
        self.file_access = _AsyncFileAccess(self)
        self.end_users = _AsyncEndUsers(self)
        self.spend = _AsyncSpend(self)
        self.admin = _AsyncAdmin(self)
        self.oauth = _AsyncOAuth(self)

    async def close(self) -> None:
        await self._http.aclose()

    async def __aenter__(self) -> "AsyncLiteGenClient":
        return self

    async def __aexit__(self, *_exc: object) -> None:
        await self.close()

    async def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        return (await self._request_with_headers(method, path, json=json, params=params))[0]

    async def _request_with_headers(
        self,
        method: str,
        path: str,
        *,
        json: Any = None,
        params: Any = None,
        content: bytes | None = None,
        headers: Mapping[str, str] | None = None,
        follow_redirects: bool = False,
    ) -> tuple[Any, httpx.Headers]:
        """:meth:`_request`, plus the response headers (the job submits read the
        dropped-params header, see :func:`litegen.client._parse_dropped_params`;
        mesh conversion sends raw ``content`` and reads its losses header)."""
        try:
            resp = await self._http.request(
                method,
                path,
                json=json,
                params=params,
                content=content,
                headers=headers,
                follow_redirects=follow_redirects,
            )
        except httpx.TimeoutException as e:
            raise LiteGenTimeoutError(str(e)) from e
        if resp.status_code >= 400:
            _raise_for_status(resp)
        ctype = resp.headers.get("content-type", "")
        if ctype.startswith("application/json"):
            return resp.json(), resp.headers
        return resp.content, resp.headers


class _AsyncImages:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def generate(self, *, idempotency_key: str | None = None, keep_outputs: bool | None = None, **req: Any) -> Any:
        """Generate images (``POST /v1/images/generations``). ``idempotency_key``
        is sent as the ``Idempotency-Key`` header, never in the body."""
        return (
            await self._c._request_with_headers(
                "POST", "/v1/images/generations", json=_clean(req), headers=_idempotency(idempotency_key, keep_outputs)
            )
        )[0]

    async def submit(
        self,
        *,
        wait_seconds: float | None = None,
        idempotency_key: str | None = None,
        keep_outputs: bool | None = None,
        **req: Any,
    ) -> Any:
        """Async version of :meth:`litegen.client._ImagesNamespace.submit`."""
        headers = _idempotency(idempotency_key, keep_outputs) or {}
        headers["Prefer"] = _prefer_async(wait_seconds)
        return (await self._c._request_with_headers("POST", "/v1/images/generations", json=_clean(req), headers=headers))[0]

    async def estimate_cost(self, **req: Any) -> Any:
        """Price an image request without running it (``POST
        /v1/images/cost``); see :meth:`litegen.client._ImagesNamespace.estimate_cost`."""
        return await self._c._request("POST", "/v1/images/cost", json=_clean(req))

    async def segment(self, *, idempotency_key: str | None = None, **req: Any) -> ImageSegmentationResponse:
        """Masks for what ``prompts`` select in ``image`` (``POST
        /v1/images/segmentations``), ``strict`` as on a generation; see
        :meth:`litegen.client._ImagesNamespace.segment`."""
        return (
            await self._c._request_with_headers(
                "POST", "/v1/images/segmentations", json=_clean(req), headers=_idempotency(idempotency_key)
            )
        )[0]


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
    memoized: the POST fires on first use. ``await job.dropped_params()`` names
    any params lax mode (``strict=False``) dropped.
    """

    def __init__(
        self,
        submit: Callable[[], Awaitable[tuple[dict[str, Any], Mapping[str, str]]]],
        get_status: Callable[[str], Awaitable[dict[str, Any]]],
        *,
        interval: float = 2.0,
        timeout: float = JOB_DEFAULT_TIMEOUT,
    ) -> None:
        # `submit` returns the response headers alongside the body because the
        # dropped-params report travels in a header.
        self._submit = submit
        self._get_status = get_status
        self._interval = interval
        self._timeout = timeout
        self._submitted: dict[str, Any] | None = None
        self._submit_error: Exception | None = None
        self._dropped_params: list[str] = []
        # One POST however many coroutines reach first use together — the same
        # guard as AsyncModel3dJob. `gather(job.submitted, job.dropped_params())`
        # would otherwise start, and bill, two videos.
        self._submit_lock = asyncio.Lock()

    async def _ensure_submitted(self) -> dict[str, Any]:
        async with self._submit_lock:
            # A failed submit is raised again, never retried: a POST that timed
            # out may still have started, and billed, the job — and the next
            # coroutine waiting on the lock would otherwise send another.
            if self._submit_error is not None:
                raise self._submit_error
            if self._submitted is None:
                try:
                    body, headers = await self._submit()
                except Exception as e:
                    self._submit_error = e
                    raise
                self._dropped_params = _parse_dropped_params(headers.get(_DROPPED_PARAMS_HEADER))
                self._submitted = body
            return self._submitted

    @property
    def submitted(self) -> Awaitable[dict[str, Any]]:
        """Awaitable resolving to the initial submit response (id, status)."""
        return self._ensure_submitted()

    async def dropped_params(self) -> list[str]:
        """What the server dropped rather than refused, because the request set
        ``strict=False``: params the model does not take and fields the request
        has no such name for (one inside an object by its path); ``[]`` when
        nothing was dropped. A last ``+N more`` counts names past the 32 the
        header lists. A method, not a property, because it submits the job
        first if it hasn't been yet."""
        await self._ensure_submitted()
        return list(self._dropped_params)

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
        timeout = req.pop("timeout", JOB_DEFAULT_TIMEOUT)
        headers = _idempotency(req.pop("idempotency_key", None), req.pop("keep_outputs", None))
        body = _clean(req)

        async def _submit() -> tuple[dict[str, Any], httpx.Headers]:
            return await self._c._request_with_headers(
                "POST", "/v1/videos/generations", json=body, headers=headers
            )

        return AsyncVideoJob(
            _submit, self.get_status, interval=interval, timeout=timeout
        )

    async def estimate_cost(self, **req: Any) -> Any:
        """Price a video request without running it (``POST
        /v1/videos/cost``); see :meth:`litegen.client._VideosNamespace.estimate_cost`."""
        return await self._c._request("POST", "/v1/videos/cost", json=_clean(req))

    async def get_status(self, video_id: str) -> Any:
        return await self._c._request("GET", f"/v1/videos/{video_id}")

    async def wait_for_completion(
        self, video_id: str, *, interval: float = 2.0, timeout: float = JOB_DEFAULT_TIMEOUT
    ) -> Any:
        return await async_wait_for_video_completion(
            video_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self, video_id: str, *, interval: float = 2.0, timeout: float = JOB_DEFAULT_TIMEOUT
    ) -> AsyncIterator[Any]:
        """Stream status updates as an async generator::

            async for update in client.videos.poll(job["id"]):
                print(update["status"], update["progress"])
        """
        return apoll_video(
            video_id, self.get_status, interval=interval, timeout=timeout
        )


class AsyncModel3dJob:
    """Awaitable, async-iterable handle returned by ``client.models3d.generate``.

    3D generation is always asynchronous on the server, so this handle mirrors
    the sync :class:`~litegen.client.Model3dJob`::

        # await → the final job (submits + polls under the hood)
        final = await client.models3d.generate(model="fal/hyper3d-rodin", prompt="a teapot")
        meshes = [a for a in final.get("assets", []) if a["kind"] == "mesh"]

        # …or stream progress (each update carries the unified 0-100 progress)
        async for update in client.models3d.generate(model="...", prompt="..."):
            print(update["status"], update["progress"])

    A ``failed`` or ``cancelled`` job is returned, not raised. ``await
    job.submitted`` returns the initial submit response without waiting, ``await
    job.id()`` its id, and ``await job.dropped_params()`` what lax mode dropped.
    Submission is lazy and memoized: the POST fires on first use.
    """

    def __init__(
        self,
        submit: Callable[[], Awaitable[tuple[Model3dGenerationResponse, Mapping[str, str]]]],
        get_status: Callable[[str], Awaitable[Model3dGenerationResponse]],
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
    ) -> None:
        self._submit = submit
        self._get_status = get_status
        self._interval = interval
        self._timeout = timeout
        self._submitted: Model3dGenerationResponse | None = None
        self._submit_error: Exception | None = None
        self._dropped_params: list[str] = []
        # One POST however many coroutines reach first use together. Every
        # accessor here is an awaitable that submits on demand, so an innocent
        # `gather(job.id(), job.dropped_params())` would otherwise start, and
        # bill, two 3D jobs.
        self._submit_lock = asyncio.Lock()

    async def _ensure_submitted(self) -> Model3dGenerationResponse:
        async with self._submit_lock:
            # A failed submit is raised again, never retried: a POST that timed
            # out may still have started, and billed, the job — and the next
            # coroutine waiting on the lock would otherwise send another.
            if self._submit_error is not None:
                raise self._submit_error
            if self._submitted is None:
                try:
                    body, headers = await self._submit()
                except Exception as e:
                    self._submit_error = e
                    raise
                self._dropped_params = _parse_dropped_params(
                    headers.get(_DROPPED_PARAMS_HEADER)
                )
                self._submitted = body
            return self._submitted

    @property
    def submitted(self) -> Awaitable[Model3dGenerationResponse]:
        """Awaitable resolving to the initial submit response (id, status)."""
        return self._ensure_submitted()

    async def id(self) -> str:
        """The generation id. Submits the job first if it hasn't been yet."""
        return (await self._ensure_submitted())["id"]

    async def dropped_params(self) -> list[str]:
        """What the server dropped rather than refused, because the request set
        ``strict=False``: params the model does not take and fields the request
        has no such name for (one inside an object by its path,
        ``animations[0].duration``); ``[]`` when nothing was dropped. A last
        ``+N more`` counts names past the 32 the header lists. Submits the job
        first if it hasn't been yet."""
        await self._ensure_submitted()
        return list(self._dropped_params)

    def __await__(self) -> Generator[Any, None, Model3dGenerationResponse]:
        return self._result().__await__()

    async def _result(self) -> Model3dGenerationResponse:
        last = await self._ensure_submitted()
        async for update in self._stream():
            last = update
        return last

    def __aiter__(self) -> AsyncIterator[Model3dGenerationResponse]:
        return self._stream()

    async def _stream(self) -> AsyncIterator[Model3dGenerationResponse]:
        job = await self._ensure_submitted()
        async for update in apoll_model3d(
            job["id"], self._get_status, interval=self._interval, timeout=self._timeout
        ):
            yield update


class _AsyncModels3d:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    def generate(
        self,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
        idempotency_key: str | None = None,
        keep_outputs: bool | None = None,
        **req: Unpack[Model3dGenerateParams],
    ) -> AsyncModel3dJob:
        """Submit a 3D job (``POST /v1/models3d/generations``), returning an
        awaitable, async-iterable :class:`AsyncModel3dJob`.

        ``await`` it for the finished job, or ``async for`` over it to stream
        progress. ``interval`` / ``timeout`` (seconds) tune the polling and are
        never sent; the timeout defaults to 30 minutes, and hitting it stops the
        polling, not the job.
        """
        body = _clean(req)
        headers = _idempotency(idempotency_key, keep_outputs)

        async def _submit() -> tuple[Model3dGenerationResponse, httpx.Headers]:
            return await self._c._request_with_headers(
                "POST", "/v1/models3d/generations", json=body, headers=headers
            )

        return AsyncModel3dJob(
            _submit, self.get_status, interval=interval, timeout=timeout
        )

    async def estimate_cost(self, **req: Unpack[Model3dGenerateParams]) -> CostEstimate:
        """Price a 3D request without running it (``POST /v1/models3d/cost``);
        see :meth:`litegen.client._Models3dNamespace.estimate_cost`."""
        return await self._c._request("POST", "/v1/models3d/cost", json=_clean(req))

    async def get_status(self, generation_id: str) -> Model3dGenerationResponse:
        """Current state of a 3D job (``GET /v1/models3d/{id}``)."""
        return await self._c._request(
            "GET", f"/v1/models3d/{quote(generation_id, safe='')}"
        )

    async def wait_for_completion(
        self,
        generation_id: str,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
    ) -> Model3dGenerationResponse:
        """Poll ``GET /v1/models3d/{id}`` until the job is terminal; return the final state."""
        return await async_wait_for_model3d_completion(
            generation_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self,
        generation_id: str,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
    ) -> AsyncIterator[Model3dGenerationResponse]:
        """Stream status updates from ``GET /v1/models3d/{id}`` as an async generator::

            async for update in client.models3d.poll(job_id):
                print(update["status"], update["progress"])
        """
        return apoll_model3d(
            generation_id, self.get_status, interval=interval, timeout=timeout
        )

    async def convert(
        self,
        data: bytes,
        *,
        from_format: Model3dFormat,
        to_format: Model3dFormat,
        units: str | None = None,
        up: str | None = None,
        from_units: str | None = None,
        from_up: str | None = None,
        fbx_color_space: str | None = None,
        package: str | None = None,
        ktx2_textures: str | None = None,
        fbx_light_units: str | None = None,
        usd_light_units: str | None = None,
        dae_morphs: str | None = None,
        usdz_root: str | None = None,
        usd_clips: str | None = None,
    ) -> Model3dConversion:
        """Convert a mesh file to another container
        (``POST /v1/utils/models3d/convert``); see
        :meth:`litegen.client._Models3dNamespace.convert`."""
        body, headers = await self._c._request_with_headers(
            "POST", "/v1/utils/models3d/convert",
            params=_convert_params(
                from_format, to_format, units, up, from_units, from_up, fbx_color_space, package, ktx2_textures,
                fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips,
            ),
            content=bytes(data),
            headers=_OCTET_STREAM,
        )
        return _conversion(body, headers, to_format, package)

    async def convert_formats(self) -> Model3dConvertFormats:
        """What the converter accepts (``GET /v1/utils/models3d/formats``)."""
        return await self._c._request("GET", "/v1/utils/models3d/formats")


class _AsyncGenerations:
    """Generation rows, shared by every media type (image, video and 3D)."""

    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def list(self, *, page: int = 1, per_page: int = 50) -> GenerationPage:
        """One page of generation rows (``GET /v1/generations``)."""
        return await self._c._request(
            "GET", "/v1/generations", params={"page": page, "per_page": per_page}
        )

    async def get(self, generation_id: str) -> Generation:
        """One generation row (``GET /v1/generations/{id}``)."""
        return await self._c._request(
            "GET", f"/v1/generations/{quote(generation_id, safe='')}"
        )

    async def wait_for_completion(
        self, generation_id: str, *, interval: float = 2.0, timeout: float = JOB_DEFAULT_TIMEOUT
    ) -> Generation:
        """Async version of :meth:`litegen.client._GenerationsNamespace.wait_for_completion`."""
        return await async_wait_for_job(generation_id, self.get, interval=interval, timeout=timeout, kind="generation")

    async def assets(self, generation_id: str) -> GenerationAssetList:
        """Every file litegen kept for a generation (``GET /v1/generations/{id}/assets``):
        its images, video, meshes, textures and previews, each with its URL, size
        and SHA-256, kept for at least 7 days whatever the vendor's URLs do."""
        return await self._c._request("GET", f"/v1/generations/{quote(generation_id, safe='')}/assets")

    async def download_asset(self, generation_id: str, index: int) -> bytes:
        """The bytes of one kept file (``GET /v1/generations/{id}/assets/{index}``),
        with only a litegen key. ``index`` is the asset's ``index`` in :meth:`assets`.
        A signed-mode app answers with a redirect to its bucket, which is followed;
        httpx drops the litegen key on the way to another host."""
        return (
            await self._c._request_with_headers(
                "GET", f"/v1/generations/{quote(generation_id, safe='')}/assets/{int(index)}", follow_redirects=True
            )
        )[0]

    async def cancel(self, generation_id: str) -> CancelledGeneration:
        """Cancel a pending or processing job (``PATCH /v1/generations/{id}``).

        Works for any async job, video or 3D, and returns the row as settled —
        a cancelled job is refunded, so ``cost_usd`` is 0 — with
        ``vendor_cancelled``, whether the vendor took the cancel. A 3D
        job's polls report ``cancelled`` from then on, so an
        :class:`AsyncModel3dJob` being awaited returns it. A job that already
        finished cannot be cancelled: the server answers 409, raised as
        :class:`~litegen.errors.LiteGenAPIError`.
        """
        return await self._c._request(
            "PATCH",
            f"/v1/generations/{quote(generation_id, safe='')}",
            json={"status": "cancelled"},
        )

    async def delete(self, generation_id: str) -> Generation:
        """Delete a finished generation's kept files and its request (prompt and
        params), keeping its billing record (``DELETE /v1/generations/{id}``)."""
        return await self._c._request("DELETE", f"/v1/generations/{quote(generation_id, safe='')}")

    async def revoke_links(self, generation_id: str) -> Any:
        """End every link litegen signed to this generation's files
        (``POST /v1/generations/{id}/revoke-links``)."""
        return await self._c._request("POST", f"/v1/generations/{quote(generation_id, safe='')}/revoke-links")

    async def download_signed(self, generation_id: str, index: int, query: str) -> bytes:
        """The bytes behind a link litegen signed, which needs no credentials."""
        return await self._c._request(
            "GET", f"/v1/generations/{quote(generation_id, safe='')}/assets/{int(index)}/signed?{query.lstrip('?')}"
        )


class _AsyncModels:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def list(self, *, media_type: MediaType | None = None, all: bool = False) -> Any:
        """The models the caller may use (``GET /v1/models``), optionally of one
        ``media_type``. ``all=True`` lists every model the gateway serves, not
        only the ones the org pool and app access allow (generations still
        enforce both)."""
        params: dict[str, str] = {}
        if media_type is not None:
            params["media_type"] = media_type
        if all:
            params["all"] = "true"
        resp = await self._c._request("GET", "/v1/models", params=params or None)
        return resp["data"] if isinstance(resp, dict) and "data" in resp else resp

    async def get(self, model_id: str) -> Any:
        """One model's full schema, ``litegen_params`` and ``extra_params``
        included; see :meth:`litegen.client._ModelsNamespace.get`."""
        return await self._c._request("GET", f"/v1/models/{model_id}")


class _AsyncHealth:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def check(self) -> Any:
        """Each provider set up with the gateway's own keys, one entry per
        provider and ``modality``; the deployment's own, so other credentials
        get a 403. See :meth:`litegen.client._HealthNamespace.check`."""
        return await self._c._request("GET", "/health")

    async def live(self) -> Any:
        return await self._c._request("GET", "/health/live")


class _AsyncStats:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def get(self, *, metadata: Mapping[str, str] | None = None) -> Any:
        """Usage stats for the caller's org (and app). ``metadata`` narrows them
        to the requests whose metadata holds every pair: spend per project or asset."""
        return await self._c._request("GET", "/v1/stats", params=_metadata_params(metadata) or None)


class _AsyncLogs:
    def __init__(self, c: AsyncLiteGenClient) -> None:
        self._c = c

    async def list(self, *, page: int = 1, per_page: int = 50, metadata: Mapping[str, str] | None = None) -> Any:
        """Request logs, newest first; ``metadata`` keeps the requests whose
        metadata holds every pair."""
        return await self._c._request(
            "GET", "/v1/logs", params={"page": page, "per_page": per_page, **_metadata_params(metadata)}
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
        """Empty the response cache; a 503 ``cache_clear_incomplete`` means only
        this replica's was. See :meth:`litegen.client._CacheNamespace.clear`."""
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


class _AsyncFileAccess:
    """Who may open an app's stored files (``/v1/apps/{id}/access``; see
    ``docs/storage-access.md``).

    ``set_app`` replaces the app's own settings: ``mode`` (``"public"``,
    ``"signed"`` or ``"private"``), ``owner_scope`` (``"key"``, ``"app"`` or
    ``"org"``) and ``signed_url_ttl_seconds`` (60 to 604800). One left out uses
    the gateway's value; ``set_app(app_id)`` clears them all. Both answer the
    app's own settings and the ones its files follow (``effective``).
    """

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def get_app(self, app_id: str) -> Any:
        return await self._c._request("GET", f"/v1/apps/{_q(app_id)}/access")

    async def set_app(
        self,
        app_id: str,
        *,
        mode: str | None = None,
        owner_scope: str | None = None,
        signed_url_ttl_seconds: int | None = None,
        keep_outputs: bool | None = None,
        retention_days: int | None = None,
        signed_links: str | None = None,
    ) -> Any:
        body = {
            "mode": mode,
            "owner_scope": owner_scope,
            "signed_url_ttl_seconds": signed_url_ttl_seconds,
            "keep_outputs": keep_outputs,
            "retention_days": retention_days,
            "signed_links": signed_links,
        }
        return await self._c._request(
            "PUT", f"/v1/apps/{_q(app_id)}/access", json={k: v for k, v in body.items() if v is not None}
        )

    async def revoke_app_links(self, app_id: str) -> Any:
        """End every link litegen signed to the app's files. A bucket's own
        presigned links work until they expire."""
        return await self._c._request("POST", f"/v1/apps/{_q(app_id)}/access/revoke-links")


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
        """Add a model at runtime; ``schema`` is a ``models/*.yaml`` row as JSON,
        or a schema ``models.get`` served, sent back as it came (its
        ``litegen_params`` and ``status`` are read and ignored)."""
        return await self._c._request("POST", "/v1/admin/models", json=dict(schema))

    async def replace(self, model_id: str, schema: Mapping[str, Any]) -> Any:
        """Replace a runtime model's schema, taken as :meth:`create` takes it."""
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
        self._c = client
        self.models = _AsyncAdminModels(client)
        self.providers = _AsyncAdminProviders(client)
        self.routes = _AsyncAdminRoutes(client)
        self.settings = _AsyncAdminSettings(client)

    async def revoke_links(self) -> Any:
        """End every link litegen signed, gateway-wide (``POST /v1/admin/revoke-links``)."""
        return await self._c._request("POST", "/v1/admin/revoke-links")


class _AsyncOAuth:
    """Sign-in for MCP clients. The consent page's API, for a signed-in
    dashboard session: read a sign-in request, then approve or deny it and send
    the browser to the returned ``redirect_to``. And the apps a person has
    connected."""

    def __init__(self, client: AsyncLiteGenClient) -> None:
        self._c = client

    async def get_request(self, request_id: str) -> Any:
        return await self._c._request("GET", f"/v1/oauth/requests/{_q(request_id)}")

    async def approve_request(self, request_id: str, *, org_id: str, scopes: list[str], app_id: str | None = None) -> Any:
        body = _drop_none({"org_id": org_id, "app_id": app_id, "scopes": scopes})
        return await self._c._request("POST", f"/v1/oauth/requests/{_q(request_id)}/approve", json=body)

    async def deny_request(self, request_id: str) -> Any:
        return await self._c._request("POST", f"/v1/oauth/requests/{_q(request_id)}/deny")

    async def list_grants(self, *, user_id: str | None = None) -> list[Any]:
        """The caller's connected apps; the platform admin names whose with ``user_id``."""
        return await self._c._request("GET", "/v1/oauth/grants", params=_drop_none({"user_id": user_id}))

    async def revoke_grant(self, grant_id: str) -> Any:
        """Disconnect an app: its tokens stop working at once."""
        return await self._c._request("DELETE", f"/v1/oauth/grants/{_q(grant_id)}")
