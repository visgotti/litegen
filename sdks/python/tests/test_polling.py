"""Tests for the polling generators: the generic `poll_job` loop and the video
and 3D wrappers built on it."""
from __future__ import annotations

import enum
import time
from typing import Any

import httpx
import pytest

from litegen.errors import LiteGenAPIError, LiteGenPollingTimeoutError, LiteGenTimeoutError
from litegen.polling import (
    apoll_job,
    apoll_video,
    async_wait_for_job,
    async_wait_for_video_completion,
    poll_job,
    poll_video,
    wait_for_job,
    wait_for_video_completion,
)


def _sync_status(seq: list[dict[str, Any]]):
    state = {"i": 0}

    def get_status(_id: str) -> dict[str, Any]:
        item = seq[min(state["i"], len(seq) - 1)]
        state["i"] += 1
        return item

    return get_status


def test_poll_video_yields_each_update() -> None:
    seq = [
        {"status": "processing", "progress": 0},
        {"status": "processing", "progress": 50},
        {"status": "completed", "progress": 100},
    ]
    updates = list(poll_video("v1", _sync_status(seq), interval=0))
    assert [u["progress"] for u in updates] == [0, 50, 100]
    assert updates[-1]["status"] == "completed"


def test_wait_for_completion_returns_terminal() -> None:
    seq = [
        {"status": "processing", "progress": 20},
        {"status": "completed", "progress": 100},
    ]
    final = wait_for_video_completion("v1", _sync_status(seq), interval=0)
    assert final["status"] == "completed"
    assert final["progress"] == 100


@pytest.mark.asyncio
async def test_apoll_video_yields_each_update() -> None:
    seq = [
        {"status": "processing", "progress": 10},
        {"status": "completed", "progress": 100},
    ]
    state = {"i": 0}

    async def get_status(_id: str) -> dict[str, Any]:
        item = seq[min(state["i"], len(seq) - 1)]
        state["i"] += 1
        return item

    updates = [u async for u in apoll_video("v1", get_status, interval=0)]
    assert [u["progress"] for u in updates] == [10, 100]

    state["i"] = 0
    final = await async_wait_for_video_completion("v1", get_status, interval=0)
    assert final["status"] == "completed"


# ─── poll_job: the one loop behind every family ──────────────────────────────


@pytest.mark.parametrize("terminal", ["completed", "failed", "cancelled"])
def test_poll_job_stops_at_the_first_terminal_status(terminal: str) -> None:
    calls: list[str] = []
    seq = [{"status": "pending"}, {"status": terminal}, {"status": "processing"}]

    def get_status(job_id: str) -> dict[str, Any]:
        calls.append(job_id)
        return seq[len(calls) - 1]

    updates = list(poll_job("j1", get_status, interval=0))
    assert [u["status"] for u in updates] == ["pending", terminal]
    assert calls == ["j1", "j1"], "no poll after the terminal status"
    assert wait_for_job("j1", _sync_status(seq[:2]), interval=0)["status"] == terminal


def test_poll_job_reads_enum_statuses_too() -> None:
    # Generated-model statuses are Enums; the loop compares their `.value`.
    class Status(enum.Enum):
        COMPLETED = "completed"

    updates = list(poll_job("j1", _sync_status([{"status": Status.COMPLETED}]), interval=0))
    assert len(updates) == 1


def test_poll_job_timeout_names_the_kind() -> None:
    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        list(
            poll_job(
                "j1",
                _sync_status([{"status": "processing"}]),
                interval=0.01,
                timeout=0.05,
                kind="3D generation",
            )
        )
    err = exc.value
    assert str(err) == "Polling for 3D generation 'j1' timed out (last status: processing)"
    assert (err.kind, err.job_id, err.video_id, err.last_status) == (
        "3D generation",
        "j1",
        "j1",
        "processing",
    )


def test_poll_job_defaults_to_the_neutral_word() -> None:
    with pytest.raises(LiteGenPollingTimeoutError, match=r"^Polling for job 'j1' timed out"):
        wait_for_job("j1", _sync_status([{"status": "pending"}]), interval=0.01, timeout=0.05)


async def test_apoll_job_and_async_wait_for_job() -> None:
    seq = [{"status": "processing", "progress": 50}, {"status": "failed", "progress": 100}]
    state = {"i": 0}

    async def get_status(_id: str) -> dict[str, Any]:
        item = seq[min(state["i"], len(seq) - 1)]
        state["i"] += 1
        return item

    updates = [u async for u in apoll_job("j1", get_status, interval=0, kind="widget")]
    assert [u["status"] for u in updates] == ["processing", "failed"]

    state["i"] = 0
    final = await async_wait_for_job("j1", get_status, interval=0)
    assert final["status"] == "failed"


async def test_apoll_job_timeout_names_the_kind() -> None:
    async def get_status(_id: str) -> dict[str, Any]:
        return {"status": "pending"}

    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        await async_wait_for_job("j1", get_status, interval=0.01, timeout=0.05, kind="widget")
    assert str(exc.value) == "Polling for widget 'j1' timed out (last status: pending)"


# ─── the video wrappers behave exactly as before ─────────────────────────────


def test_video_timeouts_still_read_as_a_video() -> None:
    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        wait_for_video_completion(
            "v1", _sync_status([{"status": "processing"}]), interval=0.01, timeout=0.05
        )
    err = exc.value
    assert str(err) == "Polling for video 'v1' timed out (last status: processing)"
    assert (err.kind, err.video_id, err.job_id) == ("video", "v1", "v1")

    with pytest.raises(LiteGenPollingTimeoutError, match=r"^Polling for video 'v1'"):
        list(poll_video("v1", _sync_status([{"status": "pending"}]), interval=0.01, timeout=0.05))


async def test_async_video_timeouts_still_read_as_a_video() -> None:
    async def get_status(_id: str) -> dict[str, Any]:
        return {"status": "processing"}

    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        await async_wait_for_video_completion("v1", get_status, interval=0.01, timeout=0.05)
    assert str(exc.value) == "Polling for video 'v1' timed out (last status: processing)"

    with pytest.raises(LiteGenPollingTimeoutError, match=r"^Polling for video 'v1'"):
        _ = [u async for u in apoll_video("v1", get_status, interval=0.01, timeout=0.05)]


def test_polling_timeout_error_keeps_its_original_two_argument_form() -> None:
    err = LiteGenPollingTimeoutError("v1", "processing")
    assert str(err) == "Polling for video 'v1' timed out (last status: processing)"
    assert (err.video_id, err.job_id, err.kind, err.last_status) == (
        "v1",
        "v1",
        "video",
        "processing",
    )
    assert str(LiteGenPollingTimeoutError("v1")) == "Polling for video 'v1' timed out"
    # The keyword it shipped with still works.
    by_keyword = LiteGenPollingTimeoutError(video_id="v1", last_status="processing")
    assert str(by_keyword) == str(err)
    assert (by_keyword.video_id, by_keyword.job_id) == ("v1", "v1")
    with pytest.raises(TypeError):
        LiteGenPollingTimeoutError("v1", video_id="v2")
    with pytest.raises(TypeError):
        LiteGenPollingTimeoutError()


# ─── transient status-read failures are ridden out ───────────────────────────


def _flaky_status(seq: list[Any]):
    """A get_status that raises each exception in ``seq`` and returns each
    mapping, in order; the last item repeats."""
    state = {"calls": 0}

    def get_status(_id: str) -> dict[str, Any]:
        item = seq[min(state["calls"], len(seq) - 1)]
        state["calls"] += 1
        if isinstance(item, Exception):
            raise item
        return item

    return get_status, state


def _transient() -> list[Exception]:
    return [
        LiteGenAPIError(429, "Rate limit exceeded", retry_after=0.0),
        LiteGenAPIError(503, "at capacity"),
        LiteGenTimeoutError("read timed out"),
        httpx.ConnectError("connection refused"),
    ]


def test_poll_job_rides_out_a_rate_limit_a_503_a_timeout_and_a_dropped_connection() -> None:
    # Over a 30-minute 3D wait one of these is likely, and raising it would
    # abandon a job that keeps running (and billing) on the server.
    seq = [*_transient(), {"status": "processing"}, *_transient(), {"status": "completed"}]
    get_status, state = _flaky_status(seq)
    updates = list(poll_job("j1", get_status, interval=0, timeout=5))
    assert [u["status"] for u in updates] == ["processing", "completed"]
    assert state["calls"] == len(seq)


@pytest.mark.parametrize("status", [400, 401, 403, 404, 409])
def test_poll_job_raises_a_client_error_at_once(status: int) -> None:
    get_status, state = _flaky_status([LiteGenAPIError(status, "no")])
    with pytest.raises(LiteGenAPIError) as exc:
        list(poll_job("j1", get_status, interval=0, timeout=5))
    assert exc.value.status == status
    assert state["calls"] == 1


def test_poll_job_gives_up_after_five_failures_in_a_row() -> None:
    get_status, state = _flaky_status([LiteGenAPIError(502, "bad gateway")])
    with pytest.raises(LiteGenAPIError) as exc:
        list(poll_job("j1", get_status, interval=0, timeout=5))
    assert exc.value.status == 502
    assert state["calls"] == 6, "the first read and five retries"


def test_poll_job_waits_out_retry_after_only_until_its_deadline() -> None:
    limited = LiteGenAPIError(429, "Rate limit exceeded", retry_after=30.0)
    get_status, _ = _flaky_status([{"status": "processing"}, limited])
    started = time.monotonic()
    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        list(poll_job("j1", get_status, interval=0, timeout=0.2))
    assert time.monotonic() - started < 2, "slept the full Retry-After past the deadline"
    assert exc.value.last_status == "processing"
    assert exc.value.__cause__ is limited


async def test_apoll_job_rides_out_transient_failures_and_gives_up_on_a_run() -> None:
    seq = [*_transient(), {"status": "completed"}]
    state = {"calls": 0}

    async def get_status(_id: str) -> dict[str, Any]:
        item = seq[min(state["calls"], len(seq) - 1)]
        state["calls"] += 1
        if isinstance(item, Exception):
            raise item
        return item

    final = await async_wait_for_job("j1", get_status, interval=0, timeout=5)
    assert final["status"] == "completed"

    async def always_down(_id: str) -> dict[str, Any]:
        state["calls"] += 1
        raise LiteGenAPIError(500, "boom")

    state["calls"] = 0
    with pytest.raises(LiteGenAPIError):
        await async_wait_for_job("j1", always_down, interval=0, timeout=5)
    assert state["calls"] == 6
