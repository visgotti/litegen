"""Polling helpers for LiteGen's async jobs (video and 3D generation).

There is no SSE on the wire: a job is submitted, then its status endpoint is
polled until the job reaches a terminal status. :func:`poll_job` (and its async
twin :func:`apoll_job`) is the one implementation of that loop. The per-family
helpers (``poll_video``, ``poll_model3d``, …) are thin instantiations of it that
fix only the word a timeout reports and, for 3D, the default timeout, so a new
async media family gets polling by wrapping it the same way.
"""
from __future__ import annotations

import asyncio
import time
from typing import (
    Any,
    AsyncIterator,
    Awaitable,
    Callable,
    Iterator,
    Mapping,
    TypeVar,
    cast,
)

import httpx

from .errors import LiteGenAPIError, LiteGenPollingTimeoutError, LiteGenTimeoutError
from .types import Model3dGenerationResponse

_TERMINAL = {"completed", "failed", "cancelled"}

# Any job response will do: the loop only ever reads its "status" key.
_JobT = TypeVar("_JobT", bound=Mapping[str, Any])

# What a 3D timeout calls itself, shared so every 3D path (the poll generator
# and the blocking wait) reports the same word.
_MODEL3D_KIND = "3D generation"

# Default client-side timeout for 3D jobs, in seconds: 30 minutes, against 5 for
# video. A vendor 3D job can sit in a queue and then refine for well past five
# minutes, and giving up early buys nothing: a client-side timeout does not stop
# the server-side job, which goes on to complete (and be billed) with nobody
# waiting for it. Thirty minutes stays well inside the backend's 2-hour reap age
# (MAX_ACTIVE_AGE_SECS in litegen-core/src/proxy/poller.rs), past which a job
# that never finished is marked failed anyway.
MODEL3D_DEFAULT_TIMEOUT = 1800.0

# Consecutive failed status reads a poll rides out before raising the last one.
# A status read is a GET, safe to repeat, and over a 30-minute 3D wait a
# rate-limit 429, a 503 while the proxy sheds load, or a dropped connection is
# likely; giving up on one throws away a job that keeps running, and billing,
# on the server.
_MAX_POLL_RETRIES = 5
_MAX_RETRY_DELAY = 30.0


def _retry_delay(err: Exception, failures: int, interval: float) -> float | None:
    """Seconds to wait before reading the status again after ``err``, the
    ``failures``-th failure in a row, or ``None`` when ``err`` is final."""
    if isinstance(err, LiteGenAPIError):
        if err.status != 429 and err.status < 500:
            return None
        if err.retry_after is not None:
            return err.retry_after
    elif not isinstance(err, (LiteGenTimeoutError, httpx.TransportError)):
        return None
    return min(interval * 2 ** (failures - 1), _MAX_RETRY_DELAY)


def poll_job(
    job_id: str,
    get_status: Callable[[str], _JobT],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
    kind: str = "job",
) -> Iterator[_JobT]:
    """Yield each status update for any async job, including the terminal one.

    ``get_status`` fetches the job's current state by id; anything that returns
    a mapping with a ``status`` key works. ``progress`` is LiteGen's unified
    0-100 value across every provider and modality, so this loop behaves the
    same for video and 3D: providers that don't report fine-grained progress
    simply step toward 100. Iteration stops after the first terminal status
    (``completed`` / ``failed`` / ``cancelled``), which is the last value
    yielded. A ``failed`` job is yielded like any other update, never raised.

    A status read that fails transiently (a 429 or 5xx, a timeout, a dropped
    connection) is retried, after the server's ``Retry-After`` when it sent
    one, up to five times in a row; any other error is raised at once.

    Raises :class:`~litegen.errors.LiteGenPollingTimeoutError`, naming ``kind``
    in its message, once ``timeout`` seconds pass without a terminal status.
    That stops the polling, not the job.
    """
    deadline = time.monotonic() + timeout
    last: _JobT | None = None
    failures = 0
    last_error: Exception | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                job_id, _status_value(last), kind=kind
            ) from last_error
        try:
            last = get_status(job_id)
        except Exception as e:
            failures += 1
            delay = _retry_delay(e, failures, interval)
            if delay is None or failures > _MAX_POLL_RETRIES:
                raise
            last_error = e
            time.sleep(min(delay, max(0.0, deadline - time.monotonic())))
            continue
        failures, last_error = 0, None
        yield last
        if _status_value(last) in _TERMINAL:
            return
        time.sleep(interval)


async def apoll_job(
    job_id: str,
    get_status: Callable[[str], Awaitable[_JobT]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
    kind: str = "job",
) -> AsyncIterator[_JobT]:
    """Async version of :func:`poll_job`: drive it with ``async for``."""
    deadline = time.monotonic() + timeout
    last: _JobT | None = None
    failures = 0
    last_error: Exception | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                job_id, _status_value(last), kind=kind
            ) from last_error
        try:
            last = await get_status(job_id)
        except Exception as e:
            failures += 1
            delay = _retry_delay(e, failures, interval)
            if delay is None or failures > _MAX_POLL_RETRIES:
                raise
            last_error = e
            await asyncio.sleep(min(delay, max(0.0, deadline - time.monotonic())))
            continue
        failures, last_error = 0, None
        yield last
        if _status_value(last) in _TERMINAL:
            return
        await asyncio.sleep(interval)


def wait_for_job(
    job_id: str,
    get_status: Callable[[str], _JobT],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
    kind: str = "job",
) -> _JobT:
    """Poll until the job reaches a terminal status, then return that final update."""
    last: _JobT | None = None
    for last in poll_job(
        job_id, get_status, interval=interval, timeout=timeout, kind=kind
    ):
        pass
    # poll_job yields at least once before it can finish without raising.
    return cast(_JobT, last)


async def async_wait_for_job(
    job_id: str,
    get_status: Callable[[str], Awaitable[_JobT]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
    kind: str = "job",
) -> _JobT:
    """Async version of :func:`wait_for_job`."""
    last: _JobT | None = None
    async for last in apoll_job(
        job_id, get_status, interval=interval, timeout=timeout, kind=kind
    ):
        pass
    # apoll_job yields at least once before it can finish without raising.
    return cast(_JobT, last)


# ─── Video ───────────────────────────────────────────────────────────────────


def poll_video(
    video_id: str,
    get_status: Callable[[str], dict[str, Any]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> Iterator[dict[str, Any]]:
    """Yield each status update for a video job, including the terminal one.

    Drive it with a ``for`` loop::

        for update in client.videos.poll(job["id"]):
            print(update["status"], update["progress"])

    ``progress`` is LiteGen's unified 0-100 value; providers that don't report
    fine-grained progress simply step toward 100, so this loop behaves the same
    across every provider. Iteration stops after the first terminal status
    (``completed`` / ``failed`` / ``cancelled``).
    """
    return poll_job(video_id, get_status, interval=interval, timeout=timeout, kind="video")


def apoll_video(
    video_id: str,
    get_status: Callable[[str], Awaitable[dict[str, Any]]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> AsyncIterator[dict[str, Any]]:
    """Async version of :func:`poll_video` — drive with ``async for``."""
    return apoll_job(video_id, get_status, interval=interval, timeout=timeout, kind="video")


def wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], dict[str, Any]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Synchronously poll until the video reaches a terminal status."""
    return wait_for_job(video_id, get_status, interval=interval, timeout=timeout, kind="video")


async def async_wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], Awaitable[dict[str, Any]]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Async version of `wait_for_video_completion`."""
    return await async_wait_for_job(
        video_id, get_status, interval=interval, timeout=timeout, kind="video"
    )


# ─── 3D ──────────────────────────────────────────────────────────────────────


def poll_model3d(
    generation_id: str,
    get_status: Callable[[str], Model3dGenerationResponse],
    *,
    interval: float = 2.0,
    timeout: float = MODEL3D_DEFAULT_TIMEOUT,
) -> Iterator[Model3dGenerationResponse]:
    """Yield each status update for a 3D generation, including the terminal one::

        for update in client.models3d.poll(job["id"]):
            print(update["status"], update["progress"])

    This is :func:`poll_job` with the 3D defaults: a 30-minute timeout
    (:data:`MODEL3D_DEFAULT_TIMEOUT`), reported as a "3D generation" when hit.
    """
    return poll_job(
        generation_id, get_status, interval=interval, timeout=timeout, kind=_MODEL3D_KIND
    )


def apoll_model3d(
    generation_id: str,
    get_status: Callable[[str], Awaitable[Model3dGenerationResponse]],
    *,
    interval: float = 2.0,
    timeout: float = MODEL3D_DEFAULT_TIMEOUT,
) -> AsyncIterator[Model3dGenerationResponse]:
    """Async version of :func:`poll_model3d`: drive it with ``async for``."""
    return apoll_job(
        generation_id, get_status, interval=interval, timeout=timeout, kind=_MODEL3D_KIND
    )


def wait_for_model3d_completion(
    generation_id: str,
    get_status: Callable[[str], Model3dGenerationResponse],
    *,
    interval: float = 2.0,
    timeout: float = MODEL3D_DEFAULT_TIMEOUT,
) -> Model3dGenerationResponse:
    """Poll until the 3D generation reaches a terminal status; return the final update."""
    return wait_for_job(
        generation_id, get_status, interval=interval, timeout=timeout, kind=_MODEL3D_KIND
    )


async def async_wait_for_model3d_completion(
    generation_id: str,
    get_status: Callable[[str], Awaitable[Model3dGenerationResponse]],
    *,
    interval: float = 2.0,
    timeout: float = MODEL3D_DEFAULT_TIMEOUT,
) -> Model3dGenerationResponse:
    """Async version of :func:`wait_for_model3d_completion`."""
    return await async_wait_for_job(
        generation_id, get_status, interval=interval, timeout=timeout, kind=_MODEL3D_KIND
    )


def _status_value(payload: Mapping[str, Any] | None) -> str | None:
    if payload is None:
        return None
    status = payload.get("status")
    return getattr(status, "value", status if isinstance(status, str) else None)
