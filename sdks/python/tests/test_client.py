"""Unit tests for the LiteGen Python SDK."""
from __future__ import annotations

import asyncio
from typing import Any

import pytest
import respx
from httpx import Response

from litegen import (
    AsyncLiteGenClient,
    AsyncVideoJob,
    LiteGenAPIError,
    LiteGenClient,
    LiteGenValidationError,
)


@respx.mock
def test_images_generate_sends_auth_and_body() -> None:
    route = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            200,
            json={
                "created": 1,
                "data": [{"url": "https://x.png", "content_type": "image/png", "index": 0}],
                "model": "m",
                "provider": "p",
                "id": "img-1",
            },
        )
    )
    client = LiteGenClient(api_key="lg-test")
    resp = client.images.generate(prompt="a cat", model="m")
    assert route.called
    sent = route.calls[0].request
    assert sent.headers["authorization"] == "Bearer lg-test"
    assert b"a cat" in sent.content
    assert resp["id"] == "img-1"


@respx.mock
def test_validation_error_decoded() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            400,
            json={
                "error": {
                    "message": "bad",
                    "type": "validation_error",
                    "code": "400",
                }
            },
        )
    )
    client = LiteGenClient(api_key="lg")
    with pytest.raises(LiteGenValidationError) as exc:
        client.images.generate(prompt="", model="m")
    assert exc.value.status == 400
    assert exc.value.type == "validation_error"


@respx.mock
def test_provider_error_raised_as_api_error_not_validation() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            502,
            json={
                "error": {
                    "message": "upstream broke",
                    "type": "provider_error",
                }
            },
        )
    )
    client = LiteGenClient()
    with pytest.raises(LiteGenAPIError) as exc:
        client.images.generate(prompt="x", model="m")
    assert exc.value.status == 502
    assert not isinstance(exc.value, LiteGenValidationError)


@respx.mock
def test_video_wait_for_completion_polls_until_done() -> None:
    statuses = ["processing", "processing", "completed"]

    def _resp(_req):
        s = statuses.pop(0)
        return Response(
            200,
            json={
                "id": "vid-1",
                "status": s,
                "model": "m",
                "provider": "p",
                "progress": 100 if s == "completed" else 50,
                "created": 1,
                "video_url": "https://done.mp4" if s == "completed" else None,
            },
        )

    respx.get("http://localhost:4000/v1/videos/vid-1").mock(side_effect=_resp)
    client = LiteGenClient()
    result = client.videos.wait_for_completion("vid-1", interval=0.01, timeout=5.0)
    assert result["status"] == "completed"
    assert result["video_url"] == "https://done.mp4"


def _mock_video_job() -> None:
    """Mock POST /generations + a GET that goes processing → completed."""
    respx.post("http://localhost:4000/v1/videos/generations").mock(
        return_value=Response(
            200,
            json={
                "id": "vid-9",
                "status": "processing",
                "model": "m",
                "provider": "p",
                "progress": 0,
                "created": 1,
            },
        )
    )
    statuses = ["processing", "completed"]

    def _status(_req):
        s = statuses.pop(0)
        return Response(
            200,
            json={
                "id": "vid-9",
                "status": s,
                "model": "m",
                "provider": "p",
                "progress": 100 if s == "completed" else 50,
                "created": 1,
                "video_url": "https://done.mp4" if s == "completed" else None,
            },
        )

    respx.get("http://localhost:4000/v1/videos/vid-9").mock(side_effect=_status)


@respx.mock
def test_video_job_result_returns_final() -> None:
    _mock_video_job()
    client = LiteGenClient()
    final = client.videos.generate(
        prompt="p", model="runway/gen-3", interval=0.0, timeout=5.0
    ).result()
    assert final["status"] == "completed"
    assert final["video_url"] == "https://done.mp4"


@respx.mock
def test_video_job_iteration_streams_progress() -> None:
    _mock_video_job()
    client = LiteGenClient()
    progress = [
        u["progress"]
        for u in client.videos.generate(
            prompt="p", model="runway/gen-3", interval=0.0, timeout=5.0
        )
    ]
    assert progress == [50, 100]


@respx.mock
def test_video_job_submitted_does_not_wait() -> None:
    _mock_video_job()
    client = LiteGenClient()
    submitted = client.videos.generate(prompt="p", model="runway/gen-3").submitted
    assert submitted["id"] == "vid-9"
    assert submitted["status"] == "processing"


_SUBMITTED_VIDEO = {
    "id": "vid-9", "status": "processing", "model": "m", "provider": "p",
    "progress": 0, "created": 1,
}


@respx.mock
def test_validation_error_names_the_param_and_model() -> None:
    # The validator's error body names the offending field and the model.
    respx.post("http://localhost:4000/v1/models3d/generations").mock(
        return_value=Response(
            400,
            json={
                "error": {
                    "code": "param_unsupported",
                    "message": "seed not supported by 'mock/mesh-3d'",
                    "model": "mock/mesh-3d",
                    "param": "seed",
                    "type": "validation_error",
                }
            },
        )
    )
    job = LiteGenClient().models3d.generate(model="mock/mesh-3d", prompt="a fox", seed=7)
    with pytest.raises(LiteGenValidationError) as exc:
        job.result()
    assert (exc.value.code, exc.value.param, exc.value.model) == (
        "param_unsupported",
        "seed",
        "mock/mesh-3d",
    )


def test_api_error_param_and_model_default_to_none() -> None:
    err = LiteGenAPIError(500, "boom", type="internal_error")
    assert err.param is None and err.model is None


@respx.mock
def test_video_job_reports_params_dropped_in_lax_mode() -> None:
    # Lax mode names what it dropped in a response header on the submit.
    submit = respx.post("http://localhost:4000/v1/videos/generations").mock(
        return_value=Response(
            200,
            json=_SUBMITTED_VIDEO,
            headers={"X-Litegen-Dropped-Params": "fps,negative_prompt"},
        )
    )
    client = LiteGenClient()
    job = client.videos.generate(prompt="p", model="runway/gen-3", strict=False, fps=60)
    assert job.dropped_params == ["fps", "negative_prompt"]  # submits on first read
    assert job.submitted["id"] == "vid-9"
    assert submit.call_count == 1

    # No header: nothing was dropped.
    submit.mock(return_value=Response(200, json=_SUBMITTED_VIDEO))
    assert client.videos.generate(prompt="p", model="runway/gen-3").dropped_params == []


@pytest.mark.asyncio
@respx.mock
async def test_async_video_job_reports_params_dropped_in_lax_mode() -> None:
    _mock_video_job()  # the status polls; the submit is re-mocked with the header
    submit = respx.post("http://localhost:4000/v1/videos/generations").mock(
        return_value=Response(
            200, json=_SUBMITTED_VIDEO, headers={"x-litegen-dropped-params": "fps"}
        )
    )
    async with AsyncLiteGenClient() as client:
        job = client.videos.generate(
            prompt="p", model="runway/gen-3", strict=False, fps=60, interval=0.0, timeout=5.0
        )
        assert await job.dropped_params() == ["fps"]
        final = await job
    assert final["status"] == "completed"
    assert submit.call_count == 1


@pytest.mark.asyncio
async def test_async_video_job_concurrent_first_use_submits_once() -> None:
    # Every accessor submits on demand, so two coroutines reaching first use
    # together must share ONE POST — otherwise this starts, and bills, two
    # videos. The submit yields mid-request, as a real one does, which is
    # exactly where an unguarded check-then-submit lets the second one in.
    calls = 0

    async def submit() -> tuple[dict[str, Any], dict[str, str]]:
        nonlocal calls
        calls += 1
        await asyncio.sleep(0.01)
        return dict(_SUBMITTED_VIDEO), {"x-litegen-dropped-params": "fps"}

    async def get_status(_id: str) -> dict[str, Any]:
        return {**_SUBMITTED_VIDEO, "status": "completed", "progress": 100}

    job = AsyncVideoJob(submit, get_status, interval=0.0, timeout=5.0)
    submitted, dropped = await asyncio.gather(job.submitted, job.dropped_params())
    assert calls == 1
    assert submitted["id"] == "vid-9"
    assert dropped == ["fps"]


@pytest.mark.asyncio
@respx.mock
async def test_async_video_job_await_returns_final() -> None:
    _mock_video_job()
    async with AsyncLiteGenClient() as client:
        final = await client.videos.generate(
            prompt="p", model="runway/gen-3", interval=0.0, timeout=5.0
        )
    assert final["status"] == "completed"
    assert final["video_url"] == "https://done.mp4"


@pytest.mark.asyncio
@respx.mock
async def test_async_video_job_streams_progress() -> None:
    _mock_video_job()
    progress: list[int] = []
    async with AsyncLiteGenClient() as client:
        async for update in client.videos.generate(
            prompt="p", model="runway/gen-3", interval=0.0, timeout=5.0
        ):
            progress.append(update["progress"])
    assert progress == [50, 100]


@pytest.mark.asyncio
@respx.mock
async def test_async_video_job_submitted_does_not_wait() -> None:
    _mock_video_job()
    async with AsyncLiteGenClient() as client:
        submitted = await client.videos.generate(prompt="p", model="runway/gen-3").submitted
    assert submitted["id"] == "vid-9"
    assert submitted["status"] == "processing"


@respx.mock
def test_models_list_unwraps_envelope() -> None:
    respx.get("http://localhost:4000/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": [{"id": "m1"}]})
    )
    client = LiteGenClient()
    models = client.models.list()
    assert models == [{"id": "m1"}]


@respx.mock
def test_keys_create_posts_name() -> None:
    route = respx.post("http://localhost:4000/v1/keys").mock(
        return_value=Response(
            201,
            json={
                "key": "lg-abc",
                "prefix": "lg-abc12",
                "name": "my-key",
                "created_at": "2026-05-28T00:00:00Z",
            },
        )
    )
    client = LiteGenClient()
    resp = client.keys.create("my-key")
    assert resp["key"] == "lg-abc"
    assert b"my-key" in route.calls[0].request.content


@respx.mock
def test_keys_list_unwraps_envelope() -> None:
    respx.get("http://localhost:4000/v1/keys").mock(
        return_value=Response(
            200,
            json={
                "data": [
                    {
                        "id": "00000000-0000-0000-0000-000000000001",
                        "name": "k1",
                        "prefix": "lg-aaa",
                        "created_at": "2026-05-28T00:00:00Z",
                        "is_active": True,
                    }
                ]
            },
        )
    )
    client = LiteGenClient()
    keys = client.keys.list()
    assert len(keys) == 1
    assert keys[0]["name"] == "k1"


@pytest.mark.asyncio
@respx.mock
async def test_async_client_generate() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            200,
            json={
                "created": 1,
                "data": [],
                "model": "m",
                "provider": "p",
                "id": "img-1",
            },
        )
    )
    async with AsyncLiteGenClient(api_key="lg") as client:
        resp = await client.images.generate(prompt="x", model="m")
    assert resp["id"] == "img-1"


@pytest.mark.asyncio
@respx.mock
async def test_async_wait_for_completion() -> None:
    statuses = ["processing", "completed"]

    def _resp(_req):
        s = statuses.pop(0)
        return Response(
            200,
            json={
                "id": "vid-2",
                "status": s,
                "model": "m",
                "provider": "p",
                "progress": 100 if s == "completed" else 30,
                "created": 1,
                "video_url": "https://v.mp4" if s == "completed" else None,
            },
        )

    respx.get("http://localhost:4000/v1/videos/vid-2").mock(side_effect=_resp)
    async with AsyncLiteGenClient() as client:
        final = await client.videos.wait_for_completion("vid-2", interval=0.01, timeout=5.0)
    assert final["status"] == "completed"
