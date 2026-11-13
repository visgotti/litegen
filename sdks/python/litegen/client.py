"""Synchronous LiteGen SDK client."""
from __future__ import annotations

import email.utils
import os
import re
import threading
import time
from typing import Any, Callable, Iterator, Mapping
from urllib.parse import quote

import httpx
from typing_extensions import Unpack

from .errors import (
    LiteGenAPIError,
    LiteGenTimeoutError,
    LiteGenValidationError,
)
from .polling import (
    JOB_DEFAULT_TIMEOUT,
    MODEL3D_DEFAULT_TIMEOUT,
    poll_model3d,
    poll_video,
    wait_for_job,
    wait_for_model3d_completion,
    wait_for_video_completion,
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
    RateLimit,
    ResponseMeta,
)

# How long a request may take by default, in seconds: 11.5 minutes. A
# synchronous call can keep the server busy for three vendor attempts of up to
# 120 s each (``server.vendor_timeout_secs``), after fetching a 3D operation's
# URL source for up to 300 s, and a client that gives up first loses an answer
# the server is still producing, and billing.
DEFAULT_TIMEOUT = 300 + 3 * 120 + 30.0


class LiteGenClient:
    """Synchronous client for the LiteGen HTTP API.

    Example:
        >>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
        >>> img = client.images.generate(prompt="a cat", model="openai/gpt-image-2")
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
        hooks = {"response": [on_response]} if on_response else None
        self._http = httpx.Client(base_url=self._base_url, headers=headers, timeout=timeout, event_hooks=hooks)

        self.images = _ImagesNamespace(self)
        self.videos = _VideosNamespace(self)
        self.models3d = _Models3dNamespace(self)
        self.generations = _GenerationsNamespace(self)
        self.models = _ModelsNamespace(self)
        self.health = _HealthNamespace(self)
        self.stats = _StatsNamespace(self)
        self.logs = _LogsNamespace(self)
        self.keys = _KeysNamespace(self)
        self.cache = _CacheNamespace(self)
        self.limits = _LimitsNamespace(self)
        self.file_access = _FileAccessNamespace(self)
        self.end_users = _EndUsersNamespace(self)
        self.spend = _SpendNamespace(self)
        self.admin = _AdminNamespace(self)
        self.oauth = _OAuthNamespace(self)

    def close(self) -> None:
        self._http.close()

    def __enter__(self) -> "LiteGenClient":
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def _request(self, method: str, path: str, *, json: Any = None, params: Any = None) -> Any:
        return self._request_with_headers(method, path, json=json, params=params)[0]

    def _request_with_headers(
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
        """:meth:`_request`, plus the response headers.

        The job submits need them (lax mode reports the params it dropped in a
        header, see :func:`_parse_dropped_params`), and so does mesh conversion,
        which also sends raw ``content`` instead of JSON.
        """
        try:
            resp = self._http.request(
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
    # The validator names the offending field and the model alongside the code.
    param = detail.get("param")
    model = detail.get("model")
    cls = LiteGenValidationError if err_type == "validation_error" else LiteGenAPIError
    raise cls(
        resp.status_code,
        message,
        type=err_type,
        code=str(code) if code is not None else None,
        provider_error=provider_error,
        param=param if isinstance(param, str) else None,
        model=model if isinstance(model, str) else None,
        retry_after=_retry_after_seconds(resp.headers.get("retry-after")),
        retryable=detail.get("retryable") if isinstance(detail.get("retryable"), bool) else None,
        headers=resp.headers,
    )


def _retry_after_seconds(value: str | None) -> float | None:
    """A ``Retry-After`` header (delta-seconds or an HTTP date) in seconds."""
    if not value:
        return None
    try:
        return max(0.0, float(value))
    except ValueError:
        pass
    try:
        at = email.utils.parsedate_to_datetime(value)
    except (TypeError, ValueError):
        return None
    return max(0.0, at.timestamp() - time.time())


def _clean(req: Mapping[str, Any]) -> dict[str, Any]:
    """Drop None values so server defaults are honored."""
    return {k: v for k, v in req.items() if v is not None}


def _prefer_async(wait_seconds: float | None) -> str:
    """The ``Prefer`` header (RFC 7240) of an image request that won't wait
    for a slow vendor job: ``respond-async``, with ``wait=N`` when the server
    may wait that long first."""
    if wait_seconds is None:
        return "respond-async"
    return f"respond-async, wait={max(0, int(wait_seconds))}"


def is_accepted(resp: Mapping[str, Any]) -> bool:
    """Whether ``images.submit`` answered with the generation to come
    (``object: "image.generation"``, status ``processing``) rather than the
    image; ``generations.wait_for_completion(resp["id"])`` waits for it."""
    return resp.get("object") == "image.generation"


def _idempotency(key: str | None, keep_outputs: bool | None = None) -> dict[str, str] | None:
    """The headers of a paid request: ``Idempotency-Key``, when a key is given,
    and ``X-Litegen-Keep-Outputs: false`` for ``keep_outputs=False``.

    The same key with the same request within 24 hours gets the first
    response back (``Idempotent-Replayed: true``) and is billed nothing; the
    same key with a different request is a 409 ``idempotency_key_reused``.
    ``keep_outputs=False`` keeps nothing for this request: no copy of the
    output is stored, and the answer carries the image itself or the vendor's
    own link. A request can keep less than its app does, never more, so
    ``True`` is refused.
    """
    if keep_outputs is True:
        raise ValueError("keep_outputs takes only False: a request can keep less than its app does, never more")
    headers: dict[str, str] = {}
    if key:
        headers["Idempotency-Key"] = key
    if keep_outputs is False:
        headers["X-Litegen-Keep-Outputs"] = "false"
    return headers or None


# Mesh conversion names what the file lost (uvs, materials, …) in this header,
# comma-joined, on its 200 — and omits it when nothing was lost.
_CONVERSION_LOSSES_HEADER = "x-litegen-conversion-losses"
# What reading the input said, "; "-joined; absent when the reader had nothing to say.
_CONVERSION_WARNINGS_HEADER = "x-litegen-conversion-warnings"
_OCTET_STREAM = {"Content-Type": "application/octet-stream"}


def _disposition_filename(value: str | None) -> str | None:
    """``model.obj`` out of ``attachment; filename="model.obj"``, if present."""
    match = re.search(r'filename="?([^";]+)"?', value or "", re.IGNORECASE)
    return match.group(1) if match else None


def _conversion(
    body: Any, headers: Mapping[str, str], to_format: Model3dFormat, package: str | None = None
) -> Model3dConversion:
    """The converted file and what its response headers say about it."""
    return {
        "content": body,
        "content_type": headers.get("content-type", "application/octet-stream"),
        "filename": _disposition_filename(headers.get("content-disposition"))
        or (f"model.{to_format}.zip" if package == "zip" else f"model.{to_format}"),
        "format": to_format,
        # Same comma-joined shape as the dropped-params header, same parser.
        "losses": _parse_dropped_params(headers.get(_CONVERSION_LOSSES_HEADER)),
        "warnings": [w for w in headers.get(_CONVERSION_WARNINGS_HEADER, "").split("; ") if w],
    }


def _convert_params(
    from_format: str,
    to_format: str,
    units: str | None,
    up: str | None,
    from_units: str | None,
    from_up: str | None,
    fbx_color_space: str | None = None,
    package: str | None = None,
    ktx2_textures: str | None = None,
    fbx_light_units: str | None = None,
    usd_light_units: str | None = None,
    dae_morphs: str | None = None,
    usdz_root: str | None = None,
    usd_clips: str | None = None,
) -> dict[str, str]:
    """The converter's query: the two containers, a frame for whichever is
    unitless (STL, OBJ, PLY), an FBX end's colour space, the packaging, what
    to do with KTX2 textures, the light units of an FBX or USD end, how a
    written Collada holds morph targets, and a written USD's root layer and
    clips, each only when asked for."""
    params = {"from": from_format, "to": to_format}
    for name, value in (
        ("units", units), ("up", up), ("from_units", from_units), ("from_up", from_up), ("fbx_color_space", fbx_color_space),
        ("package", package), ("ktx2_textures", ktx2_textures), ("fbx_light_units", fbx_light_units),
        ("usd_light_units", usd_light_units), ("dae_morphs", dae_morphs), ("usdz_root", usdz_root),
        ("usd_clips", usd_clips),
    ):
        if value is not None:
            params[name] = value
    return params


# With `strict=False` (lax mode) the server drops params the model does not
# support instead of rejecting the request, and names them in this header on the
# generation POST, comma-joined: "rig,symmetry,reference_images[view-top]". It
# is absent when nothing was dropped (litegen-core's validator `dropped_header`).
_DROPPED_PARAMS_HEADER = "x-litegen-dropped-params"


def response_meta(headers: Mapping[str, str]) -> ResponseMeta:
    """What litegen's headers say about one response: what lax mode dropped,
    the key's rate limit, whether an ``Idempotency-Key`` replayed the first
    answer, and ``Retry-After`` in seconds."""

    def number(name: str) -> int | None:
        value = (headers.get(name) or "").strip()
        return int(value) if value.isdigit() else None

    limit, remaining = number("x-ratelimit-limit"), number("x-ratelimit-remaining")
    rate_limit: RateLimit | None = None
    if limit is not None and remaining is not None:
        rate_limit = {"limit": limit, "remaining": remaining, "reset_at": number("x-ratelimit-reset")}
    return {
        "dropped_params": _parse_dropped_params(headers.get(_DROPPED_PARAMS_HEADER)),
        "rate_limit": rate_limit,
        "idempotent_replayed": (headers.get("idempotent-replayed") or "").strip().lower() == "true",
        "retry_after": _retry_after_seconds(headers.get("retry-after")),
    }


def _metadata_params(metadata: Mapping[str, str] | None) -> dict[str, str]:
    """``metadata.<key>=<value>`` for each pair a request's metadata must hold."""
    return {f"metadata.{k}": v for k, v in (metadata or {}).items()}


def _parse_dropped_params(value: str | None) -> list[str]:
    """Split an ``X-Litegen-Dropped-Params`` value into param names (``[]`` if absent)."""
    if not value:
        return []
    return [name.strip() for name in value.split(",") if name.strip()]


class _ImagesNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(self, *, idempotency_key: str | None = None, keep_outputs: bool | None = None, **req: Any) -> Any:
        """Generate images (``POST /v1/images/generations``). ``idempotency_key``
        is sent as the ``Idempotency-Key`` header, never in the body;
        ``keep_outputs=False`` keeps nothing for this request."""
        return self._c._request_with_headers(
            "POST", "/v1/images/generations", json=_clean(req), headers=_idempotency(idempotency_key, keep_outputs)
        )[0]

    def submit(
        self,
        *,
        wait_seconds: float | None = None,
        idempotency_key: str | None = None,
        keep_outputs: bool | None = None,
        **req: Any,
    ) -> Any:
        """Generate an image without waiting for a slow vendor job
        (``Prefer: respond-async``; docs/async-images.md). The answer is the
        image when the job beat ``wait_seconds`` (or the model's vendor
        answers at once), else the generation it will be delivered to — see
        :func:`is_accepted` and ``generations.wait_for_completion``."""
        headers = _idempotency(idempotency_key, keep_outputs) or {}
        headers["Prefer"] = _prefer_async(wait_seconds)
        return self._c._request_with_headers("POST", "/v1/images/generations", json=_clean(req), headers=headers)[0]

    def estimate_cost(self, **req: Any) -> Any:
        return self._c._request("POST", "/v1/images/cost", json=_clean(req))

    def segment(self, *, idempotency_key: str | None = None, **req: Any) -> ImageSegmentationResponse:
        """Masks for what ``prompts`` select in ``image`` (``POST
        /v1/images/segmentations``), answered at once. Points and boxes
        together select one object; a text prompt every instance of what it
        names. ``idempotency_key`` is sent as the ``Idempotency-Key`` header."""
        return self._c._request_with_headers(
            "POST", "/v1/images/segmentations", json=_clean(req), headers=_idempotency(idempotency_key)
        )[0]


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
    ``job.dropped_params`` names any params lax mode (``strict=False``) dropped.
    """

    def __init__(
        self,
        submit: Callable[[], tuple[dict[str, Any], Mapping[str, str]]],
        get_status: Callable[[str], dict[str, Any]],
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
        # One POST however many threads reach first use together, as on
        # AsyncVideoJob.
        self._submit_lock = threading.Lock()

    def _ensure_submitted(self) -> dict[str, Any]:
        with self._submit_lock:
            # A failed submit is raised again, never retried: a POST that timed
            # out may still have started, and billed, the job. Call
            # ``generate()`` again to submit a new one.
            if self._submit_error is not None:
                raise self._submit_error
            if self._submitted is None:
                try:
                    body, headers = self._submit()
                except Exception as e:
                    self._submit_error = e
                    raise
                self._dropped_params = _parse_dropped_params(headers.get(_DROPPED_PARAMS_HEADER))
                self._submitted = body
            return self._submitted

    @property
    def submitted(self) -> dict[str, Any]:
        """The initial submit response (id, status) — does not wait for completion."""
        return self._ensure_submitted()

    @property
    def dropped_params(self) -> list[str]:
        """Params the server dropped rather than rejected, because the request set
        ``strict=False``; ``[]`` when nothing was dropped. Submits the job first if
        it hasn't been yet."""
        self._ensure_submitted()
        return list(self._dropped_params)

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
        timeout = req.pop("timeout", JOB_DEFAULT_TIMEOUT)
        headers = _idempotency(req.pop("idempotency_key", None), req.pop("keep_outputs", None))
        body = _clean(req)
        return VideoJob(
            lambda: self._c._request_with_headers("POST", "/v1/videos/generations", json=body, headers=headers),
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
        timeout: float = JOB_DEFAULT_TIMEOUT,
    ) -> Any:
        return wait_for_video_completion(
            video_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self,
        video_id: str,
        *,
        interval: float = 2.0,
        timeout: float = JOB_DEFAULT_TIMEOUT,
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


class Model3dJob:
    """Handle returned by :meth:`_Models3dNamespace.generate`.

    3D generation is always asynchronous on the server, so, like
    :class:`VideoJob`, this handle lets you block for the finished job or
    stream progress as it runs::

        # block until the job is terminal; returns the final state
        final = client.models3d.generate(model="fal/hyper3d-rodin", prompt="a teapot").result()
        meshes = [a for a in final.get("assets", []) if a["kind"] == "mesh"]

        # …or stream progress (each update carries the unified 0-100 progress)
        for update in client.models3d.generate(model="...", prompt="..."):
            print(update["status"], update["progress"])

    A ``failed`` or ``cancelled`` job is returned like a completed one, not
    raised: check ``final["status"]``, and ``final.get("error")`` for why.

    ``job.submitted`` returns the initial submit response without waiting,
    ``job.id`` its id (e.g. for ``client.generations.cancel``), and
    ``job.dropped_params`` the params lax mode dropped. Submission is lazy and
    memoized: the POST fires on first use, which is also where a rejected
    request raises (e.g. :class:`~litegen.errors.LiteGenValidationError`).
    """

    def __init__(
        self,
        submit: Callable[[], tuple[Model3dGenerationResponse, Mapping[str, str]]],
        get_status: Callable[[str], Model3dGenerationResponse],
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
        # One POST however many threads reach first use together, as on
        # AsyncModel3dJob.
        self._submit_lock = threading.Lock()

    def _ensure_submitted(self) -> Model3dGenerationResponse:
        with self._submit_lock:
            # A failed submit is raised again, never retried: a POST that timed
            # out may still have started, and billed, the job. Call
            # ``generate()`` again to submit a new one.
            if self._submit_error is not None:
                raise self._submit_error
            if self._submitted is None:
                try:
                    body, headers = self._submit()
                except Exception as e:
                    self._submit_error = e
                    raise
                self._dropped_params = _parse_dropped_params(headers.get(_DROPPED_PARAMS_HEADER))
                self._submitted = body
            return self._submitted

    @property
    def submitted(self) -> Model3dGenerationResponse:
        """The initial submit response (id, status) — does not wait for completion."""
        return self._ensure_submitted()

    @property
    def id(self) -> str:
        """The generation id. Submits the job first if it hasn't been yet."""
        return self._ensure_submitted()["id"]

    @property
    def dropped_params(self) -> list[str]:
        """Params the server dropped rather than rejected, because the request set
        ``strict=False``; ``[]`` when nothing was dropped. Submits the job first if
        it hasn't been yet."""
        self._ensure_submitted()
        return list(self._dropped_params)

    def __iter__(self) -> Iterator[Model3dGenerationResponse]:
        return poll_model3d(
            self.submitted["id"],
            self._get_status,
            interval=self._interval,
            timeout=self._timeout,
        )

    def result(self) -> Model3dGenerationResponse:
        """Block until the job reaches a terminal status; return the final job."""
        last = self.submitted
        for last in self:
            pass
        return last


class _Models3dNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def generate(
        self,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
        idempotency_key: str | None = None,
        keep_outputs: bool | None = None,
        **req: Unpack[Model3dGenerateParams],
    ) -> Model3dJob:
        """Submit a 3D job (``POST /v1/models3d/generations``), returning a :class:`Model3dJob`.

        Call ``.result()`` to block until it is finished, or iterate the handle
        to stream progress. ``interval`` / ``timeout`` (seconds) tune the
        polling and are never sent. The timeout defaults to 30 minutes rather
        than video's 5 because vendor 3D jobs routinely take longer; hitting it
        stops the polling, not the job. ``idempotency_key`` is sent as the
        submit's ``Idempotency-Key`` header.
        """
        body = _clean(req)
        headers = _idempotency(idempotency_key, keep_outputs)
        return Model3dJob(
            lambda: self._c._request_with_headers("POST", "/v1/models3d/generations", json=body, headers=headers),
            self.get_status,
            interval=interval,
            timeout=timeout,
        )

    def estimate_cost(self, **req: Unpack[Model3dGenerateParams]) -> CostEstimate:
        """Price a 3D request without running it (``POST /v1/models3d/cost``)."""
        return self._c._request("POST", "/v1/models3d/cost", json=_clean(req))

    def get_status(self, generation_id: str) -> Model3dGenerationResponse:
        """Current state of a 3D job (``GET /v1/models3d/{id}``)."""
        return self._c._request("GET", f"/v1/models3d/{quote(generation_id, safe='')}")

    def wait_for_completion(
        self,
        generation_id: str,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
    ) -> Model3dGenerationResponse:
        """Poll ``GET /v1/models3d/{id}`` until the job is terminal; return the final state."""
        return wait_for_model3d_completion(
            generation_id, self.get_status, interval=interval, timeout=timeout
        )

    def poll(
        self,
        generation_id: str,
        *,
        interval: float = 2.0,
        timeout: float = MODEL3D_DEFAULT_TIMEOUT,
    ) -> Iterator[Model3dGenerationResponse]:
        """Stream status updates from ``GET /v1/models3d/{id}`` as a generator::

            for update in client.models3d.poll(job_id):
                print(update["status"], update["progress"])

        Yields the terminal update last.
        """
        return poll_model3d(
            generation_id, self.get_status, interval=interval, timeout=timeout
        )

    def convert(
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
        """Convert a mesh file you already have to another container
        (``POST /v1/utils/models3d/convert``). A standalone utility: no
        provider, no generation, nothing billed::

            out = client.models3d.convert(glb_bytes, from_format="glb", to_format="stl")
            Path(out["filename"]).write_bytes(out["content"])

        STL, OBJ and PLY carry no units or up axis and are written in glTF's
        frame (metres, +Y up). ``units`` (``"m"``, ``"cm"``, ``"mm"``, ``"in"``,
        ``"ft"``) and ``up`` (``"y"`` or ``"z"``) say otherwise for such a
        ``to_format`` — ``units="mm", up="z"`` is what a slicer expects — and
        ``from_units``/``from_up`` how to read such a ``from_format``. When either
        end is FBX, ``fbx_color_space`` (``"srgb"``, the default, or
        ``"linear"``) says how its material and light colours are stored.
        ``package="zip"`` answers with a zip (``application/zip``,
        ``model.<to>.zip``) holding the main file first and anything that goes
        beside it, such as an OBJ's ``.mtl`` and textures; the SDK returns it
        unopened. The default, ``"file"``, is the converted file alone.
        ``ktx2_textures`` says what happens to a KTX2 (Basis Universal)
        texture: ``"png"`` (the default) re-encodes it as PNG, so no container
        drops it and no glTF requires ``KHR_texture_basisu``, and one that
        cannot be decoded, or that would pass the 64 MiB texture budget, is
        left out and named as an ``image_formats`` loss; ``"keep"`` leaves it
        byte for byte when ``to_format`` is ``"glb"`` or ``"gltf"``, which then
        require that extension, so a reader with no KTX2 support may refuse the
        file (it suits viewers that upload KTX2 to the GPU: three.js with
        KTX2Loader, Babylon.js, Godot), and every other ``to_format`` gets PNG
        either way. ``fbx_light_units`` (when either end is FBX) and
        ``usd_light_units`` (when either end is USDZ, USDA or USDC) say what units
        light strengths are stored in at that end:
        ``"photometric"`` (the default; glTF's candela and lux, as three.js
        and ufbx read FBX and the UsdLux schema defines USD) or ``"blender"``
        (Blender's own, so the lights open in Blender as bright as the glTF
        does). ``dae_morphs`` (when ``to_format`` is ``"dae"``) says how
        Collada's morph targets (blend shapes) are written: ``"bake"`` (the
        default) applies the targets at each node's weights and drops their
        weight animation, which every Collada reader opens; ``"keep"`` writes
        them as ``<morph>`` controllers with their weight animation, which
        SceneKit (and so Xcode) and Godot read, while three.js's ColladaLoader
        and assimp refuse the whole file, so choose it only for a reader that
        opens controllers. ``usdz_root`` (when ``to_format`` is ``"usdz"``)
        says what the package's root layer, its first entry, is: ``"usda"``
        (the default) is USDA text, which ``usdchecker --arkit`` accepts and
        which ``<model-viewer>`` hands to AR Quick Look as it is; ``"usdc"`` is
        the binary crate, as Apple's own packagers write it, the package being
        ``model.usdc`` and its textures. The two read back as one scene.
        ``usd_clips`` (when ``to_format`` is ``"usda"``, ``"usdc"`` or
        ``"usdz"``) says which animation clips the USD file holds: ``"all"``
        keeps every clip as an alternative, a variant set ``Animation`` on the
        default prim with one variant per clip, the first selected, so a reader
        that composes variants can switch between them and one that does not
        plays the first (every clip's skeletal and blend-shape animation is
        kept; rigid, node motion is the first clip's only, the other clips' is
        named in the losses, and a scene whose clips are all rigid writes as
        ``"first"``); ``"first"`` keeps the first clip alone. The default is
        all for ``"usda"`` and ``"usdc"``, the first for ``"usdz"`` until AR
        Quick Look's behaviour with variants is checked on a device.

        Raises :class:`~litegen.errors.LiteGenValidationError` for a pair the
        converter does not support (``format_unsupported``; ``err.param`` says
        which side), a unit or axis option for a container with its own
        (``param_unsupported``) or a value it does not take (``param_invalid``),
        or a file that is not a ``from_format`` file (``format_mismatch``), and
        :class:`~litegen.errors.LiteGenAPIError` for one over the size cap (413)
        or a mesh it cannot read or convert (422).
        """
        body, headers = self._c._request_with_headers(
            "POST", "/v1/utils/models3d/convert",
            params=_convert_params(
                from_format, to_format, units, up, from_units, from_up, fbx_color_space, package, ktx2_textures,
                fbx_light_units, usd_light_units, dae_morphs, usdz_root, usd_clips,
            ),
            content=bytes(data),
            headers=_OCTET_STREAM,
        )
        return _conversion(body, headers, to_format, package)

    def convert_formats(self) -> Model3dConvertFormats:
        """What the converter accepts (``GET /v1/utils/models3d/formats``): the
        containers usable as both ``from_format`` and ``to_format``, and the
        largest input it takes. Read it rather than hard-coding a list."""
        return self._c._request("GET", "/v1/utils/models3d/formats")


class _GenerationsNamespace:
    """Generation rows, shared by every media type (image, video and 3D)."""

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, page: int = 1, per_page: int = 50) -> GenerationPage:
        """One page of generation rows (``GET /v1/generations``)."""
        return self._c._request(
            "GET", "/v1/generations", params={"page": page, "per_page": per_page}
        )

    def get(self, generation_id: str) -> Generation:
        """One generation row (``GET /v1/generations/{id}``)."""
        return self._c._request("GET", f"/v1/generations/{quote(generation_id, safe='')}")

    def wait_for_completion(
        self, generation_id: str, *, interval: float = 2.0, timeout: float = JOB_DEFAULT_TIMEOUT
    ) -> Generation:
        """Poll one generation until it is completed, failed or cancelled — a
        late image from ``images.submit``, or any other kind."""
        return wait_for_job(generation_id, self.get, interval=interval, timeout=timeout, kind="generation")

    def assets(self, generation_id: str) -> GenerationAssetList:
        """Every file litegen kept for a generation (``GET /v1/generations/{id}/assets``):
        its images, video, meshes, textures and previews, each with its URL, size
        and SHA-256, kept for at least 7 days whatever the vendor's URLs do."""
        return self._c._request("GET", f"/v1/generations/{quote(generation_id, safe='')}/assets")

    def download_asset(self, generation_id: str, index: int) -> bytes:
        """The bytes of one kept file (``GET /v1/generations/{id}/assets/{index}``),
        with only a litegen key. ``index`` is the asset's ``index`` in :meth:`assets`.
        A signed-mode app answers with a redirect to its bucket, which is followed;
        httpx drops the litegen key on the way to another host."""
        return (
            self._c._request_with_headers(
                "GET", f"/v1/generations/{quote(generation_id, safe='')}/assets/{int(index)}", follow_redirects=True
            )
        )[0]

    def cancel(self, generation_id: str) -> CancelledGeneration:
        """Cancel a pending or processing job (``PATCH /v1/generations/{id}``).

        Works for any async job, video or 3D, and returns the row as settled —
        a cancelled job is refunded, so ``cost_usd`` is 0 — with
        ``vendor_cancelled``, whether the vendor took the cancel. A 3D
        job's polls report ``cancelled`` from then on, so a :class:`Model3dJob`
        being waited on returns it. A job that already finished cannot be
        cancelled: the server answers 409, raised as
        :class:`~litegen.errors.LiteGenAPIError`.
        """
        return self._c._request(
            "PATCH",
            f"/v1/generations/{quote(generation_id, safe='')}",
            json={"status": "cancelled"},
        )

    def delete(self, generation_id: str) -> Generation:
        """Delete a finished generation's kept files and its request (prompt and
        params), keeping its billing record (``DELETE /v1/generations/{id}``).
        Returns the row with ``files_deleted_at`` set; a job still running is a
        409 (cancel it first)."""
        return self._c._request("DELETE", f"/v1/generations/{quote(generation_id, safe='')}")

    def revoke_links(self, generation_id: str) -> Any:
        """End every link litegen signed to this generation's files
        (``POST /v1/generations/{id}/revoke-links``). A bucket's own presigned
        links work until they expire."""
        return self._c._request("POST", f"/v1/generations/{quote(generation_id, safe='')}/revoke-links")

    def download_signed(self, generation_id: str, index: int, query: str) -> bytes:
        """The bytes behind a link litegen signed
        (``GET /v1/generations/{id}/assets/{index}/signed?…``), which needs no
        credentials. ``query`` is the link's own query string, as it came."""
        return self._c._request(
            "GET", f"/v1/generations/{quote(generation_id, safe='')}/assets/{int(index)}/signed?{query.lstrip('?')}"
        )


class _ModelsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, media_type: MediaType | None = None, all: bool = False) -> list[Any]:
        """The models the caller may use (``GET /v1/models``), optionally of one
        ``media_type``. ``all=True`` lists every model the gateway serves, not
        only the ones the org pool and app access allow (generations still
        enforce both)."""
        params: dict[str, str] = {}
        if media_type is not None:
            params["media_type"] = media_type
        if all:
            params["all"] = "true"
        resp = self._c._request("GET", "/v1/models", params=params or None)
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

    def get(self, *, metadata: Mapping[str, str] | None = None) -> Any:
        """Usage stats for the caller's org (and app). ``metadata`` narrows them
        to the requests whose metadata holds every pair: spend per project or asset."""
        return self._c._request("GET", "/v1/stats", params=_metadata_params(metadata) or None)


class _LogsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, page: int = 1, per_page: int = 50, metadata: Mapping[str, str] | None = None) -> Any:
        """Request logs, newest first; ``metadata`` keeps the requests whose
        metadata holds every pair."""
        return self._c._request("GET", "/v1/logs", params={"page": page, "per_page": per_page, **_metadata_params(metadata)})


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

    def get(self, key_id: str) -> Any:
        return self._c._request("GET", f"/v1/keys/{_q(key_id)}")

    def block(self, key_id: str) -> Any:
        """Refuse every request made with this key until unblocked."""
        return self._c._request("POST", f"/v1/keys/{_q(key_id)}/block")

    def unblock(self, key_id: str) -> Any:
        return self._c._request("POST", f"/v1/keys/{_q(key_id)}/unblock")


class _CacheNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def clear(self) -> Any:
        return self._c._request("DELETE", "/v1/cache")


# ─── Management: limits, end users, spend, gateway admin ─────────────────────


def _q(value: str) -> str:
    """Percent-encode one path segment."""
    return quote(value, safe="")


def _drop_none(d: Mapping[str, Any]) -> dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}


class _LimitsNamespace:
    """Budgets and limits (``/v1/limits``, ``…/limits`` on keys, orgs, apps, members).

    A limits body (``LimitsInput``) takes ``max_budget_usd``, ``soft_budget_usd``,
    ``budget_duration`` (``30s``/``30m``/``12h``/``30d``/``1w``/``1mo``),
    ``allowed_models``, ``max_parallel_requests``, ``rpm_limit`` and ``metadata``.
    ``set_*`` replaces every field; ``reset_*`` zeroes the current window's spend.
    """

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, entity_type: str | None = None, org_id: str | None = None) -> list[Any]:
        resp = self._c._request(
            "GET", "/v1/limits", params=_drop_none({"entity_type": entity_type, "org_id": org_id})
        )
        return resp["data"]

    def me(self) -> Any:
        """The limits that apply to the caller: its key, org, app and member rows."""
        return self._c._request("GET", "/v1/limits/me")["data"]

    def get_key(self, key_id: str) -> Any:
        return self._c._request("GET", f"/v1/keys/{_q(key_id)}/limits")

    def set_key(self, key_id: str, limits: Mapping[str, Any]) -> Any:
        return self._c._request("PUT", f"/v1/keys/{_q(key_id)}/limits", json=dict(limits))

    def delete_key(self, key_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/keys/{_q(key_id)}/limits")

    def reset_key(self, key_id: str) -> Any:
        return self._c._request("POST", f"/v1/keys/{_q(key_id)}/limits/reset")

    def get_org(self, org_id: str) -> Any:
        return self._c._request("GET", f"/v1/orgs/{_q(org_id)}/limits")

    def set_org(self, org_id: str, limits: Mapping[str, Any]) -> Any:
        return self._c._request("PUT", f"/v1/orgs/{_q(org_id)}/limits", json=dict(limits))

    def delete_org(self, org_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/orgs/{_q(org_id)}/limits")

    def reset_org(self, org_id: str) -> Any:
        return self._c._request("POST", f"/v1/orgs/{_q(org_id)}/limits/reset")

    def get_app(self, app_id: str) -> Any:
        return self._c._request("GET", f"/v1/apps/{_q(app_id)}/limits")

    def set_app(self, app_id: str, limits: Mapping[str, Any]) -> Any:
        return self._c._request("PUT", f"/v1/apps/{_q(app_id)}/limits", json=dict(limits))

    def delete_app(self, app_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/apps/{_q(app_id)}/limits")

    def reset_app(self, app_id: str) -> Any:
        return self._c._request("POST", f"/v1/apps/{_q(app_id)}/limits/reset")

    def get_member(self, org_id: str, user_id: str) -> Any:
        return self._c._request("GET", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits")

    def set_member(self, org_id: str, user_id: str, limits: Mapping[str, Any]) -> Any:
        return self._c._request(
            "PUT", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits", json=dict(limits)
        )

    def delete_member(self, org_id: str, user_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits")

    def reset_member(self, org_id: str, user_id: str) -> Any:
        return self._c._request("POST", f"/v1/orgs/{_q(org_id)}/members/{_q(user_id)}/limits/reset")


class _FileAccessNamespace:
    """Who may open an app's stored files (``/v1/apps/{id}/access``; see
    ``docs/storage-access.md``).

    ``set_app`` replaces the app's own settings: ``mode`` (``"public"``,
    ``"signed"`` or ``"private"``), ``owner_scope`` (``"key"``, ``"app"`` or
    ``"org"``) and ``signed_url_ttl_seconds`` (60 to 604800). One left out uses
    the gateway's value; ``set_app(app_id)`` clears them all. Both answer the
    app's own settings and the ones its files follow (``effective``).
    """

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get_app(self, app_id: str) -> Any:
        return self._c._request("GET", f"/v1/apps/{_q(app_id)}/access")

    def set_app(
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
        return self._c._request(
            "PUT", f"/v1/apps/{_q(app_id)}/access", json={k: v for k, v in body.items() if v is not None}
        )

    def revoke_app_links(self, app_id: str) -> Any:
        """End every link litegen signed to the app's files. A bucket's own
        presigned links work until they expire."""
        return self._c._request("POST", f"/v1/apps/{_q(app_id)}/access/revoke-links")


class _EndUsersNamespace:
    """End users (your callers' customers), named per request with ``x-litegen-end-user``.

    ``org_id`` is for the platform admin only, to act on another org.
    """

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self, *, org_id: str | None = None) -> list[Any]:
        return self._c._request("GET", "/v1/end-users", params=_drop_none({"org_id": org_id}))["data"]

    def create(
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
        return self._c._request(
            "POST", "/v1/end-users", json=body, params=_drop_none({"org_id": org_id})
        )

    def get(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "GET", f"/v1/end-users/{_q(end_user_id)}", params=_drop_none({"org_id": org_id})
        )

    def update(
        self,
        end_user_id: str,
        *,
        alias: str | None = None,
        blocked: bool | None = None,
        metadata: Mapping[str, Any] | None = None,
        org_id: str | None = None,
    ) -> Any:
        body = _drop_none({"alias": alias, "blocked": blocked, "metadata": metadata})
        return self._c._request(
            "PATCH", f"/v1/end-users/{_q(end_user_id)}", json=body, params=_drop_none({"org_id": org_id})
        )

    def delete(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "DELETE", f"/v1/end-users/{_q(end_user_id)}", params=_drop_none({"org_id": org_id})
        )

    def block(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/block", params=_drop_none({"org_id": org_id})
        )

    def unblock(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/unblock", params=_drop_none({"org_id": org_id})
        )

    def get_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "GET", f"/v1/end-users/{_q(end_user_id)}/limits", params=_drop_none({"org_id": org_id})
        )

    def set_limits(self, end_user_id: str, limits: Mapping[str, Any], *, org_id: str | None = None) -> Any:
        return self._c._request(
            "PUT",
            f"/v1/end-users/{_q(end_user_id)}/limits",
            json=dict(limits),
            params=_drop_none({"org_id": org_id}),
        )

    def delete_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "DELETE", f"/v1/end-users/{_q(end_user_id)}/limits", params=_drop_none({"org_id": org_id})
        )

    def reset_limits(self, end_user_id: str, *, org_id: str | None = None) -> Any:
        return self._c._request(
            "POST", f"/v1/end-users/{_q(end_user_id)}/limits/reset", params=_drop_none({"org_id": org_id})
        )


class _SpendNamespace:
    """Spend reports (``/v1/spend/report``) and the attributed request log (``/v1/spend/logs``).

    Filters: ``from_``/``to`` (RFC 3339 or ``YYYY-MM-DD``), ``org_id`` (platform
    admin only), ``app_id``, ``key_id``, ``user_id``, ``end_user``, ``model``,
    ``provider``, ``tag``.
    """

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    @staticmethod
    def _filters(from_: str | None, filters: Mapping[str, Any]) -> dict[str, Any]:
        return _drop_none({"from": from_, **filters})

    def report(self, *, group_by: str = "day", from_: str | None = None, **filters: Any) -> Any:
        params = self._filters(from_, filters)
        params["group_by"] = group_by
        return self._c._request("GET", "/v1/spend/report", params=params)

    def logs(self, *, page: int = 1, per_page: int = 50, from_: str | None = None, **filters: Any) -> Any:
        params = self._filters(from_, filters)
        params.update(page=page, per_page=per_page)
        return self._c._request("GET", "/v1/spend/logs", params=params)


class _AdminModelsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self) -> list[Any]:
        """Every model, catalog and runtime, enabled or not."""
        return self._c._request("GET", "/v1/admin/models")["data"]

    def get(self, model_id: str) -> Any:
        return self._c._request("GET", f"/v1/admin/models/{model_id}")

    def create(self, schema: Mapping[str, Any]) -> Any:
        """Add a model at runtime; ``schema`` is a ``models/*.yaml`` row as JSON."""
        return self._c._request("POST", "/v1/admin/models", json=dict(schema))

    def replace(self, model_id: str, schema: Mapping[str, Any]) -> Any:
        return self._c._request("PUT", f"/v1/admin/models/{model_id}", json=dict(schema))

    def set_disabled(self, model_id: str, disabled: bool) -> Any:
        return self._c._request("PATCH", f"/v1/admin/models/{model_id}", json={"disabled": disabled})

    def delete(self, model_id: str) -> Any:
        return self._c._request("DELETE", f"/v1/admin/models/{model_id}")


class _AdminProvidersNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def list(self) -> list[Any]:
        """Every provider adapter and its effective config; secrets are masked."""
        return self._c._request("GET", "/v1/admin/providers")["data"]

    def get(self, name: str) -> Any:
        return self._c._request("GET", f"/v1/admin/providers/{_q(name)}")

    def configure(self, name: str, **config: Any) -> Any:
        """Set ``api_key``, ``api_keys``, ``key_id``, ``key_secret``, ``api_base``,
        ``model_mapping``, ``extra_headers``, ``options``, ``region`` or ``enabled``."""
        return self._c._request("PUT", f"/v1/admin/providers/{_q(name)}", json=config)

    def reset(self, name: str) -> Any:
        """Drop the runtime config; the provider falls back to its env/YAML config."""
        return self._c._request("DELETE", f"/v1/admin/providers/{_q(name)}")


class _AdminRoutesNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get(self) -> Any:
        return self._c._request("GET", "/v1/admin/routes")

    def replace(self, routes: list[Mapping[str, Any]]) -> Any:
        return self._c._request("PUT", "/v1/admin/routes", json={"routes": [dict(r) for r in routes]})

    def reset(self) -> Any:
        return self._c._request("DELETE", "/v1/admin/routes")


class _AdminSettingsNamespace:
    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get(self) -> Any:
        return self._c._request("GET", "/v1/admin/settings")

    def update(self, **settings: Any) -> Any:
        """Replace the gateway settings; an omitted field is cleared."""
        return self._c._request("PUT", "/v1/admin/settings", json=settings)


class _AdminNamespace:
    """Gateway admin: runtime models, provider configs, routes and settings."""

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client
        self.models = _AdminModelsNamespace(client)
        self.providers = _AdminProvidersNamespace(client)
        self.routes = _AdminRoutesNamespace(client)
        self.settings = _AdminSettingsNamespace(client)

    def revoke_links(self) -> Any:
        """End every link litegen signed, gateway-wide (``POST /v1/admin/revoke-links``)."""
        return self._c._request("POST", "/v1/admin/revoke-links")


class _OAuthNamespace:
    """Sign-in for MCP clients. The consent page's API, for a signed-in
    dashboard session: read a sign-in request, then approve or deny it and send
    the browser to the returned ``redirect_to``. And the apps a person has
    connected."""

    def __init__(self, client: LiteGenClient) -> None:
        self._c = client

    def get_request(self, request_id: str) -> Any:
        return self._c._request("GET", f"/v1/oauth/requests/{_q(request_id)}")

    def approve_request(self, request_id: str, *, org_id: str, scopes: list[str], app_id: str | None = None) -> Any:
        body = _drop_none({"org_id": org_id, "app_id": app_id, "scopes": scopes})
        return self._c._request("POST", f"/v1/oauth/requests/{_q(request_id)}/approve", json=body)

    def deny_request(self, request_id: str) -> Any:
        return self._c._request("POST", f"/v1/oauth/requests/{_q(request_id)}/deny")

    def list_grants(self, *, user_id: str | None = None) -> list[Any]:
        """The caller's connected apps; the platform admin names whose with ``user_id``."""
        return self._c._request("GET", "/v1/oauth/grants", params=_drop_none({"user_id": user_id}))

    def revoke_grant(self, grant_id: str) -> Any:
        """Disconnect an app: its tokens stop working at once."""
        return self._c._request("DELETE", f"/v1/oauth/grants/{_q(grant_id)}")
