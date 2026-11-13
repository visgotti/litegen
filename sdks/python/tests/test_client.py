"""Unit tests for the LiteGen Python SDK."""
from __future__ import annotations

import asyncio
from typing import Any

import pytest
import respx
from httpx import ReadTimeout, Response

from litegen import (
    AsyncLiteGenClient,
    AsyncVideoJob,
    LiteGenAPIError,
    LiteGenClient,
    LiteGenTimeoutError,
    LiteGenValidationError,
    is_accepted,
)


@respx.mock
def test_images_submit_asks_not_to_wait_and_says_what_came_back() -> None:
    route = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            202,
            json={"id": "litegen-img-1", "object": "image.generation", "status": "processing", "model": "m", "created": 1},
        )
    )
    client = LiteGenClient(api_key="lg-test")
    resp = client.images.submit(prompt="a cat", model="m", wait_seconds=5)
    assert route.calls[0].request.headers["prefer"] == "respond-async, wait=5"
    assert is_accepted(resp) and resp["id"] == "litegen-img-1"
    client.images.submit(prompt="a cat", model="m")
    assert route.calls[1].request.headers["prefer"] == "respond-async"
    assert not is_accepted({"created": 1, "data": [], "model": "m", "provider": "p", "id": "img-1"})


@respx.mock
def test_generations_wait_for_completion_polls_the_row() -> None:
    answers = iter([{"id": "litegen-img-1", "status": "processing"}, {"id": "litegen-img-1", "status": "completed"}])
    respx.get("http://localhost:4000/v1/generations/litegen-img-1").mock(side_effect=lambda _req: Response(200, json=next(answers)))
    client = LiteGenClient(api_key="lg-test")
    done = client.generations.wait_for_completion("litegen-img-1", interval=0.01, timeout=5)
    assert done["status"] == "completed"


@respx.mock
def test_async_images_submit_and_wait() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(202, json={"id": "litegen-img-2", "object": "image.generation", "status": "processing", "model": "m", "created": 1})
    )
    answers = iter([{"id": "litegen-img-2", "status": "completed"}])
    respx.get("http://localhost:4000/v1/generations/litegen-img-2").mock(side_effect=lambda _req: Response(200, json=next(answers)))

    async def run() -> None:
        client = AsyncLiteGenClient(api_key="lg-test")
        accepted = await client.images.submit(prompt="a cat", model="m")
        assert is_accepted(accepted)
        done = await client.generations.wait_for_completion(accepted["id"], interval=0.01, timeout=5)
        assert done["status"] == "completed"

    asyncio.run(run())


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
def test_a_vendor_rejection_carries_the_vendors_answer_and_whether_to_retry() -> None:
    respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(
            400,
            json={
                "error": {
                    "message": "fal rejected the request for model fal/flux-dev: prompt flagged",
                    "type": "vendor_rejected",
                    "code": "vendor_rejected",
                    "retryable": False,
                    "provider_error": {"status": 422, "code": "content_policy_violation", "message": "prompt flagged"},
                }
            },
        )
    )
    client = LiteGenClient()
    with pytest.raises(LiteGenAPIError) as exc:
        client.images.generate(prompt="x", model="fal/flux-dev")
    assert (exc.value.code, exc.value.retryable) == ("vendor_rejected", False)
    assert exc.value.provider_error["code"] == "content_policy_violation"


def _error(status: int, error: dict[str, Any], retry_after: str | None = None) -> Response:
    return Response(status, json={"error": error}, headers={"Retry-After": retry_after} if retry_after else None)


# The router's and storage's answers from the 2026-11 review, as the SDK raises them.
_ROUTER_ANSWERS = [
    pytest.param(
        # Every deployment of a model route has its circuit breaker open.
        "POST", "/v1/images/generations", lambda c: c.images.generate(model="fal/flux-dev", prompt="x"),
        _error(503, {"message": "every deployment is cooling down", "type": "service_unavailable",
                     "code": "circuit_open", "retryable": True}, "12"),
        LiteGenAPIError, {"status": 503, "code": "circuit_open", "retryable": True, "retry_after": 12.0},
        id="circuit_open",
    ),
    pytest.param(
        # Only this replica's cache was emptied; the shared one in Redis was not.
        "DELETE", "/v1/cache", lambda c: c.cache.clear(),
        _error(503, {"message": "Only this replica's cache was emptied.", "type": "cache_clear_incomplete",
                     "code": "cache_clear_incomplete"}, "1"),
        LiteGenAPIError, {"status": 503, "code": "cache_clear_incomplete", "retryable": None, "retry_after": 1.0},
        id="cache_clear_incomplete",
    ),
    pytest.param(
        # The adapter refused the request before any vendor call.
        "POST", "/v1/videos/cost", lambda c: c.videos.estimate_cost(model="minimax/MiniMax-Hailuo-02", prompt="x"),
        _error(400, {"message": "10 s at 1080P is not offered", "type": "validation_error", "code": "request_unsupported",
                     "model": "minimax/MiniMax-Hailuo-02", "retryable": False}),
        LiteGenValidationError, {"status": 400, "code": "request_unsupported", "retryable": False, "model": "minimax/MiniMax-Hailuo-02"},
        id="request_unsupported",
    ),
    pytest.param(
        "POST", "/v1/images/generations", lambda c: c.images.generate(model="vidu/viduq2", prompt="x"),
        _error(400, {"message": "Provider 'vidu' is not configured for this application.",
                     "type": "provider_not_configured", "code": "provider_not_configured"}),
        LiteGenAPIError, {"status": 400, "code": "provider_not_configured", "retryable": None},
        id="provider_not_configured",
    ),
    pytest.param(
        # A text-to-video request with no prompt, on a model whose vendor needs one.
        "POST", "/v1/videos/cost", lambda c: c.videos.estimate_cost(model="runway/veo3.1"),
        _error(400, {"message": "prompt is required", "type": "validation_error", "code": "prompt_required",
                     "param": "prompt", "model": "runway/veo3.1"}),
        LiteGenValidationError, {"status": 400, "code": "prompt_required", "param": "prompt"},
        id="prompt_required",
    ),
    pytest.param(
        # A last frame alone, to a model whose vendor ends a clip on one only
        # after a first frame (its `last_frame` role's `needs_first_frame`).
        "POST", "/v1/videos/cost",
        lambda c: c.videos.estimate_cost(
            model="runway/veo3.1", prompt="x",
            reference_images=[{"type": "url", "value": "https://cdn.example/end.png", "role": "last_frame"}],
        ),
        _error(400, {"message": "'runway/veo3.1' ends a clip on a last_frame only after a first_frame; send a first_frame image too",
                     "type": "validation_error", "code": "ref_role_required", "param": "reference_images",
                     "model": "runway/veo3.1"}),
        LiteGenValidationError, {"status": 400, "code": "ref_role_required", "param": "reference_images"},
        id="ref_role_required_last_frame",
    ),
    pytest.param(
        # A model route none of whose deployments' vendors was called: some
        # were skipped. The video route documents this 502, as the image one does.
        "POST", "/v1/videos/generations", lambda c: c.videos.generate(model="fal/ltx-2.3", prompt="x").submitted,
        _error(502, {"message": "All deployments failed for model fal/ltx-2.3: fal: circuit open; replicate: not configured",
                     "type": "provider_error", "code": "provider_error", "retryable": True}),
        LiteGenAPIError, {"status": 502, "code": "provider_error", "retryable": True},
        id="video_route_skipped_502",
    ),
    pytest.param(
        "GET", "/v1/generations/litegen-vid-1/assets/0", lambda c: c.generations.download_asset("litegen-vid-1", 0),
        _error(416, {"message": "the requested range lies outside the file", "type": "invalid_request_error",
                     "code": "range_not_satisfiable"}),
        LiteGenAPIError, {"status": 416, "code": "range_not_satisfiable"},
        id="range_not_satisfiable",
    ),
]


@pytest.mark.parametrize(("method", "path", "call", "response", "cls", "want"), _ROUTER_ANSWERS)
def test_router_and_storage_answers_reach_the_caller(
    method: str, path: str, call: Any, response: Response, cls: type, want: dict[str, Any]
) -> None:
    with respx.mock:
        respx.route(method=method, url=f"http://localhost:4000{path}").mock(return_value=response)
        with pytest.raises(LiteGenAPIError) as exc:
            call(LiteGenClient())
    assert type(exc.value) is cls
    assert {name: getattr(exc.value, name) for name in want} == want


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


@respx.mock
def test_video_job_never_resubmits_after_a_failed_submit() -> None:
    # A timed-out POST may still have started (and billed) the video.
    route = respx.post("http://localhost:4000/v1/videos/generations").mock(
        side_effect=ReadTimeout("read timed out")
    )
    job = LiteGenClient().videos.generate(prompt="p", model="runway/gen-3")
    for _ in range(3):
        with pytest.raises(LiteGenTimeoutError):
            job.result()
    assert route.call_count == 1


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
async def test_async_video_job_failed_submit_is_not_resent_by_the_next_waiter() -> None:
    calls = 0

    async def submit() -> tuple[dict[str, Any], dict[str, str]]:
        nonlocal calls
        calls += 1
        await asyncio.sleep(0.01)
        raise LiteGenTimeoutError("read timed out")

    async def get_status(_id: str) -> dict[str, Any]:
        raise AssertionError("never submitted, so never polled")

    job = AsyncVideoJob(submit, get_status, interval=0.0, timeout=5.0)
    outcomes = await asyncio.gather(job.submitted, job.dropped_params(), return_exceptions=True)
    assert all(isinstance(o, LiteGenTimeoutError) for o in outcomes), outcomes
    with pytest.raises(LiteGenTimeoutError):
        await job
    assert calls == 1


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
def test_models_list_all_asks_for_every_served_model() -> None:
    route = respx.get("http://localhost:4000/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": []})
    )
    client = LiteGenClient()
    client.models.list(all=True, media_type="video")
    params = route.calls.last.request.url.params
    assert (params.get("all"), params.get("media_type")) == ("true", "video")
    client.models.list()
    assert "all" not in route.calls.last.request.url.params


@pytest.mark.asyncio
@respx.mock
async def test_async_models_list_all_asks_for_every_served_model() -> None:
    route = respx.get("http://localhost:4000/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": []})
    )
    async with AsyncLiteGenClient() as client:
        await client.models.list(all=True)
    assert route.calls.last.request.url.params.get("all") == "true"


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


# ─── Idempotency-Key ──────────────────────────────────────────────────────────
# One key per paid request: a retry after a crash with the same key gets the
# first response back and is billed nothing. Sent on the submit only.

_JOB_3D = {"id": "litegen-3d-1", "status": "completed", "model": "m", "provider": "p",
           "progress": 100, "created": 1,
           "assets": [{"kind": "mesh", "url": "https://x/model.glb", "format": "glb"}]}
_JOB_VID = {"id": "litegen-vid-1", "status": "completed", "model": "m", "provider": "p",
            "progress": 100, "created": 1, "video_url": "https://x.mp4"}


@respx.mock
def test_idempotency_key_is_sent_on_every_generate_submit() -> None:
    img = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={"created": 1, "data": [], "model": "m", "provider": "p", "id": "i"})
    )
    m3d = respx.post("http://localhost:4000/v1/models3d/generations").mock(return_value=Response(200, json=_JOB_3D))
    vid = respx.post("http://localhost:4000/v1/videos/generations").mock(return_value=Response(200, json=_JOB_VID))
    respx.get("http://localhost:4000/v1/models3d/litegen-3d-1").mock(return_value=Response(200, json=_JOB_3D))
    respx.get("http://localhost:4000/v1/videos/litegen-vid-1").mock(return_value=Response(200, json=_JOB_VID))
    client = LiteGenClient()
    client.images.generate(model="mock/image-gen", prompt="x", idempotency_key="ledger-img")
    client.models3d.generate(model="mock/mesh-3d", prompt="x", idempotency_key="ledger-3d", interval=0).result()
    client.videos.generate(model="mock/video-gen", prompt="x", idempotency_key="ledger-vid", interval=0).result()
    assert img.calls[0].request.headers["idempotency-key"] == "ledger-img"
    assert m3d.calls[0].request.headers["idempotency-key"] == "ledger-3d"
    assert vid.calls[0].request.headers["idempotency-key"] == "ledger-vid"
    for route in (img, m3d, vid):
        assert b"idempotency" not in route.calls[0].request.content, "never part of the body"


@respx.mock
def test_no_idempotency_key_header_unless_given() -> None:
    img = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={"created": 1, "data": [], "model": "m", "provider": "p", "id": "i"})
    )
    LiteGenClient().images.generate(model="mock/image-gen", prompt="x")
    assert "idempotency-key" not in img.calls[0].request.headers


@respx.mock
async def test_async_idempotency_key_is_sent_on_every_generate_submit() -> None:
    img = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={"created": 1, "data": [], "model": "m", "provider": "p", "id": "i"})
    )
    m3d = respx.post("http://localhost:4000/v1/models3d/generations").mock(return_value=Response(200, json=_JOB_3D))
    vid = respx.post("http://localhost:4000/v1/videos/generations").mock(return_value=Response(200, json=_JOB_VID))
    respx.get("http://localhost:4000/v1/models3d/litegen-3d-1").mock(return_value=Response(200, json=_JOB_3D))
    respx.get("http://localhost:4000/v1/videos/litegen-vid-1").mock(return_value=Response(200, json=_JOB_VID))
    async with AsyncLiteGenClient() as client:
        await client.images.generate(model="mock/image-gen", prompt="x", idempotency_key="a-img")
        await client.models3d.generate(model="mock/mesh-3d", prompt="x", idempotency_key="a-3d", interval=0)
        await client.videos.generate(model="mock/video-gen", prompt="x", idempotency_key="a-vid", interval=0)
    assert img.calls[0].request.headers["idempotency-key"] == "a-img"
    assert m3d.calls[0].request.headers["idempotency-key"] == "a-3d"
    assert vid.calls[0].request.headers["idempotency-key"] == "a-vid"


_SEGMENTATION = {
    "id": "litegen-seg-1",
    "created": 1,
    "model": "mock/segment-image",
    "provider": "mock",
    "masks": [{"png_base64": "iVBORw0KGgo=", "score": 0.9, "box": {"x": 1, "y": 2, "w": 3, "h": 4}}],
}


@respx.mock
def test_images_segment_posts_the_prompts_and_returns_the_masks() -> None:
    route = respx.post("http://localhost:4000/v1/images/segmentations").mock(return_value=Response(200, json=_SEGMENTATION))
    prompts = [{"type": "point", "x": 4, "y": 5, "label": 1}, {"type": "box", "x": 1, "y": 2, "w": 3, "h": 4}]
    resp = LiteGenClient().images.segment(model="mock/segment-image", image="iVBORw0KGgo=", prompts=prompts, idempotency_key="seg-1")
    assert resp["masks"][0]["box"] == {"x": 1, "y": 2, "w": 3, "h": 4}
    sent = route.calls[0].request
    assert sent.headers["idempotency-key"] == "seg-1"
    import json as _json
    assert _json.loads(sent.content) == {"model": "mock/segment-image", "image": "iVBORw0KGgo=", "prompts": prompts}


@pytest.mark.asyncio
@respx.mock
async def test_async_images_segment_posts_the_prompts() -> None:
    route = respx.post("http://localhost:4000/v1/images/segmentations").mock(return_value=Response(200, json=_SEGMENTATION))
    async with AsyncLiteGenClient() as client:
        resp = await client.images.segment(model="mock/segment-image", image="x", prompts=[{"type": "text", "text": "visor"}])
    assert resp["id"] == "litegen-seg-1"
    assert "idempotency-key" not in route.calls[0].request.headers


# ─── Keeping nothing, deleting and revoking (docs/storage-access.md) ─────────


@respx.mock
def test_keep_outputs_false_is_sent_on_every_generate_submit() -> None:
    img = respx.post("http://localhost:4000/v1/images/generations").mock(
        return_value=Response(200, json={"created": 1, "data": [], "model": "m", "provider": "p", "id": "i"})
    )
    m3d = respx.post("http://localhost:4000/v1/models3d/generations").mock(return_value=Response(200, json=_JOB_3D))
    vid = respx.post("http://localhost:4000/v1/videos/generations").mock(return_value=Response(200, json=_JOB_VID))
    respx.get("http://localhost:4000/v1/models3d/litegen-3d-1").mock(return_value=Response(200, json=_JOB_3D))
    respx.get("http://localhost:4000/v1/videos/litegen-vid-1").mock(return_value=Response(200, json=_JOB_VID))
    client = LiteGenClient()
    client.images.generate(model="mock/image-gen", prompt="x", keep_outputs=False)
    client.models3d.generate(model="mock/mesh-3d", prompt="x", keep_outputs=False, interval=0).result()
    client.videos.generate(model="mock/video-gen", prompt="x", keep_outputs=False, interval=0).result()
    for route in (img, m3d, vid):
        assert route.calls[0].request.headers["x-litegen-keep-outputs"] == "false"
        assert b"keep_outputs" not in route.calls[0].request.content, "never part of the body"
    client.images.generate(model="mock/image-gen", prompt="x")
    assert "x-litegen-keep-outputs" not in img.calls[1].request.headers
    with pytest.raises(ValueError):
        client.images.generate(model="mock/image-gen", prompt="x", keep_outputs=True)


@respx.mock
def test_deleting_and_revoking_use_their_routes() -> None:
    revoked = {"revoked_at": "2026-11-03T12:00:00.000Z"}
    gen = respx.post("http://localhost:4000/v1/generations/g1/revoke-links").mock(return_value=Response(200, json=revoked))
    app = respx.post("http://localhost:4000/v1/apps/a1/access/revoke-links").mock(return_value=Response(200, json=revoked))
    gateway = respx.post("http://localhost:4000/v1/admin/revoke-links").mock(return_value=Response(200, json=revoked))
    delete = respx.delete("http://localhost:4000/v1/generations/g1").mock(
        return_value=Response(200, json={"id": "g1", "files_deleted_at": "2026-11-03T12:00:00Z"})
    )
    client = LiteGenClient()
    assert client.generations.revoke_links("g1") == revoked
    assert client.file_access.revoke_app_links("a1") == revoked
    assert client.admin.revoke_links() == revoked
    assert client.generations.delete("g1")["files_deleted_at"]
    assert gen.called and app.called and gateway.called and delete.called


@respx.mock
def test_download_asset_follows_a_signed_redirect_without_the_key() -> None:
    # In signed mode the file route answers 307 to a presigned bucket URL.
    respx.get("http://localhost:4000/v1/generations/g1/assets/0").mock(
        return_value=Response(307, headers={"location": "https://bucket.example/litegen/g1/image.png?sig=x"})
    )
    bucket = respx.get("https://bucket.example/litegen/g1/image.png").mock(
        return_value=Response(200, content=b"\x89PNG-bytes", headers={"content-type": "image/png"})
    )
    client = LiteGenClient(api_key="lg-secret")
    assert client.generations.download_asset("g1", 0) == b"\x89PNG-bytes"
    assert "authorization" not in bucket.calls.last.request.headers


@pytest.mark.asyncio
@respx.mock
async def test_async_download_asset_follows_a_signed_redirect() -> None:
    respx.get("http://localhost:4000/v1/generations/g1/assets/0").mock(
        return_value=Response(307, headers={"location": "https://bucket.example/litegen/g1/image.png?sig=x"})
    )
    respx.get("https://bucket.example/litegen/g1/image.png").mock(
        return_value=Response(200, content=b"bytes", headers={"content-type": "image/png"})
    )
    async with AsyncLiteGenClient(api_key="lg-secret") as client:
        assert await client.generations.download_asset("g1", 0) == b"bytes"


# ─── health.ready() (follow-up FU-PYHEALTH) ──────────────────────────────────

_READY = {"status": "ready", "checks": {"db": True, "providers": ["fal", "openai"]}}
_NOT_READY = {"status": "not_ready", "checks": {"db": False, "providers": []}}


@respx.mock
def test_health_ready_reads_the_readiness_probe_and_raises_on_503() -> None:
    route = respx.get("http://localhost:4000/health/ready").mock(
        side_effect=[Response(200, json=_READY), Response(503, json=_NOT_READY)]
    )
    client = LiteGenClient()
    assert client.health.ready() == _READY
    with pytest.raises(LiteGenAPIError) as exc:
        client.health.ready()
    assert exc.value.status == 503
    assert [call.request.url.path for call in route.calls] == ["/health/ready", "/health/ready"]


@respx.mock
async def test_async_health_ready_reads_the_readiness_probe_and_raises_on_503() -> None:
    respx.get("http://localhost:4000/health/ready").mock(
        side_effect=[Response(200, json=_READY), Response(503, json=_NOT_READY)]
    )
    async with AsyncLiteGenClient() as client:
        assert await client.health.ready() == _READY
        with pytest.raises(LiteGenAPIError) as exc:
            await client.health.ready()
    assert exc.value.status == 503
