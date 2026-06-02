"""Tests for the video polling generators."""
from __future__ import annotations

from typing import Any

import pytest

from litegen.polling import (
    apoll_video,
    async_wait_for_video_completion,
    poll_video,
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
