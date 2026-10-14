"""Tests for the polling generators: the generic `poll_job` loop and the video
and 3D wrappers built on it."""
from __future__ import annotations

import enum
from typing import Any

import pytest

from litegen.errors import LiteGenPollingTimeoutError
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
