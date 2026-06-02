"""Polling helpers for async video generation."""
from __future__ import annotations

import asyncio
import time
from typing import Any, AsyncIterator, Awaitable, Callable, Iterator

from .errors import LiteGenPollingTimeoutError

_TERMINAL = {"completed", "failed", "cancelled"}


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
    deadline = time.monotonic() + timeout
    last: dict[str, Any] | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                video_id, _status_value(last) if last is not None else None
            )
        last = get_status(video_id)
        yield last
        if _status_value(last) in _TERMINAL:
            return
        time.sleep(interval)


async def apoll_video(
    video_id: str,
    get_status: Callable[[str], Awaitable[dict[str, Any]]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> AsyncIterator[dict[str, Any]]:
    """Async version of :func:`poll_video` — drive with ``async for``."""
    deadline = time.monotonic() + timeout
    last: dict[str, Any] | None = None
    while True:
        if time.monotonic() > deadline:
            raise LiteGenPollingTimeoutError(
                video_id, _status_value(last) if last is not None else None
            )
        last = await get_status(video_id)
        yield last
        if _status_value(last) in _TERMINAL:
            return
        await asyncio.sleep(interval)


def wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], dict[str, Any]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Synchronously poll until the video reaches a terminal status."""
    last: dict[str, Any] = {}
    for last in poll_video(video_id, get_status, interval=interval, timeout=timeout):
        pass
    return last


async def async_wait_for_video_completion(
    video_id: str,
    get_status: Callable[[str], Awaitable[dict[str, Any]]],
    *,
    interval: float = 2.0,
    timeout: float = 300.0,
) -> dict[str, Any]:
    """Async version of `wait_for_video_completion`."""
    last: dict[str, Any] = {}
    async for last in apoll_video(
        video_id, get_status, interval=interval, timeout=timeout
    ):
        pass
    return last


def _status_value(payload: dict[str, Any] | None) -> str | None:
    if payload is None:
        return None
    status = payload.get("status")
    return getattr(status, "value", status if isinstance(status, str) else None)
