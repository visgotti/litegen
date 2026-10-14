"""Unit tests for 3D generation (`client.models3d`), the shared `client.generations`
rows, the `media_type` filter on `client.models.list`, and the lax-mode
dropped-params report. Sync and async clients both; every HTTP call is mocked
with respx.
"""
from __future__ import annotations

import asyncio
import inspect
import json
from typing import Any

import pytest
import respx
from httpx import Response

from litegen import (
    MODEL3D_DEFAULT_TIMEOUT,
    AsyncLiteGenClient,
    AsyncModel3dJob,
    LiteGenAPIError,
    LiteGenClient,
    LiteGenPollingTimeoutError,
    LiteGenValidationError,
    Model3dJob,
    apoll_model3d,
    async_wait_for_model3d_completion,
    poll_model3d,
    wait_for_model3d_completion,
)
from litegen.async_client import _AsyncModels3d
from litegen.client import _Models3dNamespace, _parse_dropped_params

BASE = "http://localhost:4000"
JOB_ID = "litegen-3d-abc123"
SUBMIT_URL = f"{BASE}/v1/models3d/generations"
STATUS_URL = f"{BASE}/v1/models3d/{JOB_ID}"

GLB = {
    "kind": "mesh",
    "url": "https://cdn.test/litegen/3d/abc/model.glb",
    "format": "glb",
    "size_bytes": 2048,
    "polycount": 12,
}
OBJ = {"kind": "mesh", "url": "https://cdn.test/litegen/3d/abc/model.obj", "format": "obj"}
PREVIEW = {
    "kind": "preview",
    "url": "https://cdn.test/litegen/3d/abc/preview.png",
    "format": "png",
    "width": 512,
    "height": 512,
}
COST = {
    "base_cost_usd": 0.4,
    "markup_usd": 0.0,
    "total_cost_usd": 0.4,
    "tokens_required": 400,
    "cost_source": "estimated",
}
ROW = {
    "id": JOB_ID,
    "model": "mock/mesh-3d",
    "provider": "mock",
    "media_type": "model3d",
    "status": "processing",
    "progress": 33,
    "cost_usd": 0.0,
    "created_at": "2026-10-12T00:00:00Z",
    "key_id": None,
    "provider_job_id": "mock-job-1",
    "result_url": None,
    "error_message": None,
    "completed_at": None,
    "metadata": None,
}
VALIDATION_400 = {
    "error": {
        "type": "validation_error",
        "code": "param_unsupported",
        "message": "rig not supported by 'mock/mesh-3d'",
        "param": "rig",
        "model": "mock/mesh-3d",
    }
}


def _job(status: str, progress: int, **extra: Any) -> dict[str, Any]:
    return {
        "id": JOB_ID,
        "status": status,
        "model": "mock/mesh-3d",
        "provider": "mock",
        "progress": progress,
        "created": 1,
        **extra,
    }


def _mock_job(*updates: dict[str, Any], headers: dict[str, str] | None = None):
    """Mock the submit (answers `pending`) and a status endpoint that walks `updates`."""
    submit = respx.post(SUBMIT_URL).mock(
        return_value=Response(200, json=_job("pending", 0), headers=headers or {})
    )
    status = respx.get(STATUS_URL).mock(
        side_effect=[Response(200, json=u) for u in updates]
    )
    return submit, status


# A full run: two in-flight polls, then completed with one mesh per requested
# container plus a preview.
RUN = (
    _job("processing", 33),
    _job("processing", 66),
    _job("completed", 100, assets=[GLB, OBJ, PREVIEW]),
)


# ─── dropped-params parsing ──────────────────────────────────────────────────


@pytest.mark.parametrize(
    ("value", "expected"),
    [
        (None, []),  # header absent: nothing was dropped
        ("", []),
        ("   ", []),
        ("rig", ["rig"]),
        (
            "rig,symmetry,reference_images[view-top]",
            ["rig", "symmetry", "reference_images[view-top]"],
        ),
        (" rig , symmetry ,, ", ["rig", "symmetry"]),
    ],
)
def test_parse_dropped_params(value: str | None, expected: list[str]) -> None:
    assert _parse_dropped_params(value) == expected


# ─── sync ────────────────────────────────────────────────────────────────────


@respx.mock
def test_generate_posts_the_cleaned_body_without_the_polling_knobs() -> None:
    submit, _ = _mock_job(*RUN)
    job = LiteGenClient(api_key="lg-test").models3d.generate(
        model="mock/mesh-3d",
        prompt="a small cube",
        output_formats=["glb", "obj"],
        texture=True,
        seed=None,  # None means "the model's default": stripped, never sent as null
        reference_images=[{"type": "url", "value": "https://x.test/cat.png", "role": "init"}],
        interval=0.0,
        timeout=5.0,
    )
    assert isinstance(job, Model3dJob)
    assert not submit.called, "submission is lazy: nothing is sent before first use"

    assert job.submitted["status"] == "pending"
    sent = submit.calls[0].request
    assert sent.headers["authorization"] == "Bearer lg-test"
    assert json.loads(sent.content) == {
        "model": "mock/mesh-3d",
        "prompt": "a small cube",
        "output_formats": ["glb", "obj"],
        "texture": True,
        "reference_images": [{"type": "url", "value": "https://x.test/cat.png", "role": "init"}],
    }


@respx.mock
def test_an_image_only_request_needs_no_prompt() -> None:
    submit, _ = _mock_job(*RUN)
    LiteGenClient().models3d.generate(
        model="fal/trellis",
        reference_images=[{"type": "url", "value": "https://x.test/chair.png", "role": "init"}],
    ).submitted
    assert "prompt" not in json.loads(submit.calls[0].request.content)


@respx.mock
def test_result_polls_until_completed_and_returns_the_assets() -> None:
    submit, status = _mock_job(*RUN)
    job = LiteGenClient().models3d.generate(model="mock/mesh-3d", prompt="a cube", interval=0.0)
    final = job.result()

    assert final["status"] == "completed"
    assert status.call_count == 3
    assert submit.call_count == 1
    meshes = [a for a in final.get("assets", []) if a["kind"] == "mesh"]
    assert [m["format"] for m in meshes] == ["glb", "obj"]
    assert job.id == JOB_ID


@respx.mock
def test_iterating_the_job_yields_every_update_including_the_terminal_one() -> None:
    _mock_job(*RUN)
    updates = list(
        LiteGenClient().models3d.generate(model="mock/mesh-3d", prompt="a cube", interval=0.0)
    )
    assert [(u["status"], u["progress"]) for u in updates] == [
        ("processing", 33),
        ("processing", 66),
        ("completed", 100),
    ]


@pytest.mark.parametrize(
    "terminal",
    [
        _job("failed", 100, error="mock 3d generation failed (mock/fail-3d always fails)"),
        _job("cancelled", 33),
    ],
    ids=["failed", "cancelled"],
)
@respx.mock
def test_a_failed_or_cancelled_job_is_returned_not_raised(terminal: dict[str, Any]) -> None:
    _, status = _mock_job(_job("processing", 33), terminal, _job("completed", 100, assets=[GLB]))
    final = LiteGenClient().models3d.generate(
        model="mock/fail-3d", prompt="x", interval=0.0
    ).result()
    assert final == terminal
    assert "assets" not in final
    assert status.call_count == 2, "polling stops at the first terminal status"


@respx.mock
def test_estimate_cost_posts_the_same_body_to_the_cost_endpoint() -> None:
    route = respx.post(f"{BASE}/v1/models3d/cost").mock(return_value=Response(200, json=COST))
    est = LiteGenClient().models3d.estimate_cost(
        model="fal/hyper3d-rodin", prompt="a teapot", output_formats=["glb"], rig=None
    )
    assert est["total_cost_usd"] == 0.4
    assert json.loads(route.calls[0].request.content) == {
        "model": "fal/hyper3d-rodin",
        "prompt": "a teapot",
        "output_formats": ["glb"],
    }


@respx.mock
def test_get_status_and_wait_for_completion() -> None:
    respx.get(STATUS_URL).mock(
        side_effect=[
            Response(200, json=_job("processing", 40)),
            Response(200, json=_job("processing", 80)),
            Response(200, json=_job("completed", 100, assets=[GLB])),
        ]
    )
    client = LiteGenClient()
    assert client.models3d.get_status(JOB_ID)["progress"] == 40
    final = client.models3d.wait_for_completion(JOB_ID, interval=0.0)
    assert final["status"] == "completed"
    assert final.get("assets") == [GLB]


@respx.mock
def test_poll_streams_updates_for_an_existing_id() -> None:
    respx.get(STATUS_URL).mock(side_effect=[Response(200, json=u) for u in RUN])
    progress = [u["progress"] for u in LiteGenClient().models3d.poll(JOB_ID, interval=0.0)]
    assert progress == [33, 66, 100]


@respx.mock
def test_ids_are_url_encoded_into_the_path() -> None:
    status = respx.get(url__startswith=f"{BASE}/v1/models3d/").mock(
        return_value=Response(200, json=_job("processing", 1))
    )
    row = respx.get(url__startswith=f"{BASE}/v1/generations/").mock(
        return_value=Response(200, json=ROW)
    )
    client = LiteGenClient()
    client.models3d.get_status("odd/id ?x")
    client.generations.get("odd/id ?x")
    assert status.calls[0].request.url.raw_path == b"/v1/models3d/odd%2Fid%20%3Fx"
    assert row.calls[0].request.url.raw_path == b"/v1/generations/odd%2Fid%20%3Fx"


@respx.mock
def test_a_poll_timeout_names_a_3d_generation() -> None:
    _mock_job(*[_job("processing", 10)] * 1000)
    job = LiteGenClient().models3d.generate(
        model="mock/mesh-3d", prompt="x", interval=0.01, timeout=0.05
    )
    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        job.result()
    err = exc.value
    assert str(err) == f"Polling for 3D generation '{JOB_ID}' timed out (last status: processing)"
    assert err.kind == "3D generation"
    assert err.job_id == JOB_ID
    assert err.video_id == JOB_ID  # back-compat alias
    assert err.last_status == "processing"


@respx.mock
def test_wait_for_completion_timeout_names_a_3d_generation() -> None:
    respx.get(STATUS_URL).mock(return_value=Response(200, json=_job("pending", 0)))
    with pytest.raises(LiteGenPollingTimeoutError) as exc:
        LiteGenClient().models3d.wait_for_completion(JOB_ID, interval=0.01, timeout=0.05)
    assert exc.value.kind == "3D generation"
    assert "3D generation" in str(exc.value)


@respx.mock
def test_a_rejected_submit_raises_when_the_job_is_first_used() -> None:
    route = respx.post(SUBMIT_URL).mock(return_value=Response(400, json=VALIDATION_400))
    job = LiteGenClient().models3d.generate(model="mock/mesh-3d", prompt="x", rig=True)
    assert not route.called
    with pytest.raises(LiteGenValidationError) as exc:
        job.result()
    assert exc.value.status == 400
    assert exc.value.code == "param_unsupported"


@respx.mock
def test_the_job_reports_params_dropped_in_lax_mode() -> None:
    submit, _ = _mock_job(
        *RUN, headers={"X-Litegen-Dropped-Params": "rig,symmetry,reference_images[view-top]"}
    )
    job = LiteGenClient().models3d.generate(
        model="mock/mesh-3d", prompt="x", strict=False, rig=True, symmetry="on"
    )
    # Reading it submits the job, exactly once.
    assert job.dropped_params == ["rig", "symmetry", "reference_images[view-top]"]
    assert job.submitted["id"] == JOB_ID
    assert job.dropped_params == ["rig", "symmetry", "reference_images[view-top]"]
    assert submit.call_count == 1
    assert json.loads(submit.calls[0].request.content)["strict"] is False


@respx.mock
def test_nothing_dropped_means_an_empty_list() -> None:
    _mock_job(*RUN)
    assert LiteGenClient().models3d.generate(model="mock/mesh-3d", prompt="x").dropped_params == []


@respx.mock
def test_models_list_sends_media_type_only_when_given() -> None:
    route = respx.get(f"{BASE}/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": [{"id": "mock/mesh-3d"}]})
    )
    client = LiteGenClient()
    assert client.models.list(media_type="model3d") == [{"id": "mock/mesh-3d"}]
    client.models.list()
    assert dict(route.calls[0].request.url.params) == {"media_type": "model3d"}
    assert route.calls[1].request.url.query == b""


@respx.mock
def test_generations_list_get_and_cancel() -> None:
    page = {"data": [ROW], "total": 1, "page": 2, "per_page": 10, "total_pages": 1}
    listed = respx.get(f"{BASE}/v1/generations").mock(return_value=Response(200, json=page))
    respx.get(f"{BASE}/v1/generations/{JOB_ID}").mock(return_value=Response(200, json=ROW))
    cancel = respx.patch(f"{BASE}/v1/generations/{JOB_ID}").mock(
        return_value=Response(200, json={**ROW, "status": "cancelled"})
    )
    client = LiteGenClient()

    assert client.generations.list(page=2, per_page=10) == page
    assert dict(listed.calls[0].request.url.params) == {"page": "2", "per_page": "10"}
    assert client.generations.get(JOB_ID)["media_type"] == "model3d"
    assert client.generations.cancel(JOB_ID)["status"] == "cancelled"
    assert json.loads(cancel.calls[0].request.content) == {"status": "cancelled"}


@respx.mock
def test_cancelling_a_finished_job_is_an_api_error() -> None:
    respx.patch(f"{BASE}/v1/generations/{JOB_ID}").mock(
        return_value=Response(
            409,
            json={
                "error": {
                    "message": "Generation cannot be cancelled: not in pending or processing state",
                    "type": "api_error",
                    "code": 409,
                }
            },
        )
    )
    with pytest.raises(LiteGenAPIError) as exc:
        LiteGenClient().generations.cancel(JOB_ID)
    assert exc.value.status == 409
    assert not isinstance(exc.value, LiteGenValidationError)


def test_every_3d_path_defaults_to_the_30_minute_timeout() -> None:
    assert MODEL3D_DEFAULT_TIMEOUT == 1800.0
    for fn in (
        poll_model3d,
        apoll_model3d,
        wait_for_model3d_completion,
        async_wait_for_model3d_completion,
        Model3dJob.__init__,
        AsyncModel3dJob.__init__,
        _Models3dNamespace.generate,
        _Models3dNamespace.wait_for_completion,
        _Models3dNamespace.poll,
        _AsyncModels3d.generate,
        _AsyncModels3d.wait_for_completion,
        _AsyncModels3d.poll,
    ):
        default = inspect.signature(fn).parameters["timeout"].default
        assert default == MODEL3D_DEFAULT_TIMEOUT, fn.__qualname__


# ─── async ───────────────────────────────────────────────────────────────────


@respx.mock
async def test_async_generate_posts_the_cleaned_body_and_awaits_to_completion() -> None:
    submit, status = _mock_job(*RUN)
    async with AsyncLiteGenClient() as client:
        job = client.models3d.generate(
            model="mock/mesh-3d",
            prompt="a small cube",
            output_formats=["glb", "obj"],
            negative_prompt=None,
            interval=0.0,
            timeout=5.0,
        )
        assert isinstance(job, AsyncModel3dJob)
        assert not submit.called
        final = await job
    assert final["status"] == "completed"
    assert [a["format"] for a in final.get("assets", []) if a["kind"] == "mesh"] == ["glb", "obj"]
    assert status.call_count == 3
    assert json.loads(submit.calls[0].request.content) == {
        "model": "mock/mesh-3d",
        "prompt": "a small cube",
        "output_formats": ["glb", "obj"],
    }


@respx.mock
async def test_async_iteration_yields_every_update_including_the_terminal_one() -> None:
    _mock_job(*RUN)
    async with AsyncLiteGenClient() as client:
        updates = [
            u
            async for u in client.models3d.generate(
                model="mock/mesh-3d", prompt="a cube", interval=0.0
            )
        ]
    assert [u["progress"] for u in updates] == [33, 66, 100]
    assert updates[-1]["status"] == "completed"


@respx.mock
async def test_async_failed_job_is_returned_not_raised() -> None:
    failed = _job("failed", 100, error="mock 3d generation failed")
    _mock_job(_job("processing", 50), failed)
    async with AsyncLiteGenClient() as client:
        final = await client.models3d.generate(model="mock/fail-3d", prompt="x", interval=0.0)
    assert final["status"] == "failed"
    assert final.get("error") == "mock 3d generation failed"


@respx.mock
async def test_async_estimate_status_wait_and_poll() -> None:
    respx.post(f"{BASE}/v1/models3d/cost").mock(return_value=Response(200, json=COST))
    respx.get(STATUS_URL).mock(
        side_effect=[Response(200, json=u) for u in (_job("processing", 40), *RUN, *RUN)]
    )
    async with AsyncLiteGenClient() as client:
        assert (await client.models3d.estimate_cost(model="mock/mesh-3d", prompt="x"))[
            "cost_source"
        ] == "estimated"
        assert (await client.models3d.get_status(JOB_ID))["progress"] == 40
        final = await client.models3d.wait_for_completion(JOB_ID, interval=0.0)
        assert final["status"] == "completed"
        progress = [u["progress"] async for u in client.models3d.poll(JOB_ID, interval=0.0)]
    assert progress == [33, 66, 100]


@respx.mock
async def test_async_poll_timeout_names_a_3d_generation() -> None:
    _mock_job(*[_job("processing", 10)] * 1000)
    async with AsyncLiteGenClient() as client:
        with pytest.raises(LiteGenPollingTimeoutError) as exc:
            await client.models3d.generate(
                model="mock/mesh-3d", prompt="x", interval=0.01, timeout=0.05
            )
    assert str(exc.value) == (
        f"Polling for 3D generation '{JOB_ID}' timed out (last status: processing)"
    )
    assert (exc.value.kind, exc.value.job_id, exc.value.video_id) == (
        "3D generation",
        JOB_ID,
        JOB_ID,
    )


@respx.mock
async def test_async_rejected_submit_raises_when_the_job_is_first_used() -> None:
    respx.post(SUBMIT_URL).mock(return_value=Response(400, json=VALIDATION_400))
    async with AsyncLiteGenClient() as client:
        job = client.models3d.generate(model="mock/mesh-3d", prompt="x", rig=True)
        with pytest.raises(LiteGenValidationError):
            await job


@respx.mock
async def test_async_job_id_and_dropped_params_submit_exactly_once() -> None:
    submit, _ = _mock_job(*RUN, headers={"x-litegen-dropped-params": "rig, symmetry"})
    async with AsyncLiteGenClient() as client:
        job = client.models3d.generate(model="mock/mesh-3d", prompt="x", strict=False, rig=True)
        # Concurrent first use must not start (and bill) a second job.
        job_id, dropped, submitted = await asyncio.gather(
            job.id(), job.dropped_params(), job.submitted
        )
        assert await job.dropped_params() == ["rig", "symmetry"]
    assert (job_id, dropped, submitted["id"]) == (JOB_ID, ["rig", "symmetry"], JOB_ID)
    assert submit.call_count == 1


@respx.mock
async def test_async_models_list_and_generations() -> None:
    models = respx.get(f"{BASE}/v1/models").mock(
        return_value=Response(200, json={"object": "list", "data": [{"id": "mock/mesh-3d"}]})
    )
    page = {"data": [ROW], "total": 1, "page": 1, "per_page": 50, "total_pages": 1}
    listed = respx.get(f"{BASE}/v1/generations").mock(return_value=Response(200, json=page))
    respx.get(f"{BASE}/v1/generations/{JOB_ID}").mock(return_value=Response(200, json=ROW))
    cancel = respx.patch(f"{BASE}/v1/generations/{JOB_ID}").mock(
        return_value=Response(200, json={**ROW, "status": "cancelled"})
    )
    async with AsyncLiteGenClient() as client:
        assert await client.models.list(media_type="model3d") == [{"id": "mock/mesh-3d"}]
        await client.models.list()
        assert await client.generations.list() == page
        assert (await client.generations.get(JOB_ID))["id"] == JOB_ID
        assert (await client.generations.cancel(JOB_ID))["status"] == "cancelled"
    assert dict(models.calls[0].request.url.params) == {"media_type": "model3d"}
    assert models.calls[1].request.url.query == b""
    assert dict(listed.calls[0].request.url.params) == {"page": "1", "per_page": "50"}
    assert json.loads(cancel.calls[0].request.content) == {"status": "cancelled"}


# ─── Mesh conversion (`/v1/utils/models3d/*`) ────────────────────────────────

CONVERT_URL = f"{BASE}/v1/utils/models3d/convert"
FORMATS_URL = f"{BASE}/v1/utils/models3d/formats"
GLB_BYTES = b"glTF\x02\x00\x00\x00"  # magic + version 2: enough for a mocked server


def _obj_response(**headers: str) -> Response:
    return Response(200, content=b"o cube\nv 0.5 -0.5 0.5\n", headers={"content-type": "model/obj", **headers})


@respx.mock
def test_convert_posts_raw_bytes_with_from_and_to_in_the_query() -> None:
    route = respx.post(CONVERT_URL).mock(
        return_value=_obj_response(**{
            "content-disposition": 'attachment; filename="model.obj"',
            "x-litegen-conversion-losses": "materials,uvs",
        })
    )
    out = LiteGenClient().models3d.convert(GLB_BYTES, from_format="glb", to_format="obj")
    sent = route.calls[0].request
    assert dict(sent.url.params) == {"from": "glb", "to": "obj"}
    assert sent.headers["content-type"] == "application/octet-stream"
    assert sent.content == GLB_BYTES  # the file itself, never JSON
    assert out == {
        "content": b"o cube\nv 0.5 -0.5 0.5\n",
        "content_type": "model/obj",
        "filename": "model.obj",
        "format": "obj",
        "losses": ["materials", "uvs"],
    }


@respx.mock
def test_convert_without_loss_or_disposition_headers() -> None:
    # Losses are only sent when something was lost; the name falls back to the
    # server's own pattern.
    respx.post(CONVERT_URL).mock(return_value=_obj_response())
    out = LiteGenClient().models3d.convert(bytearray(GLB_BYTES), from_format="glb", to_format="stl")
    assert out["losses"] == [] and out["filename"] == "model.stl" and out["format"] == "stl"


@respx.mock
def test_convert_rejection_names_the_param() -> None:
    respx.post(CONVERT_URL).mock(
        return_value=Response(
            400,
            json={"error": {
                "code": "format_unsupported",
                "message": "`to` 'fbx' cannot be converted; supported: glb, obj, stl, ply",
                "param": "to",
                "type": "validation_error",
            }},
        )
    )
    with pytest.raises(LiteGenValidationError) as exc:
        LiteGenClient().models3d.convert(GLB_BYTES, from_format="glb", to_format="fbx")
    assert (exc.value.status, exc.value.code, exc.value.param) == (400, "format_unsupported", "to")


@respx.mock
def test_convert_formats_reads_the_converter_capabilities() -> None:
    respx.get(FORMATS_URL).mock(
        return_value=Response(200, json={"formats": ["glb", "obj", "stl", "ply"], "max_input_bytes": 134217728})
    )
    caps = LiteGenClient().models3d.convert_formats()
    assert caps == {"formats": ["glb", "obj", "stl", "ply"], "max_input_bytes": 134217728}


@respx.mock
async def test_async_convert_and_formats() -> None:
    route = respx.post(CONVERT_URL).mock(return_value=_obj_response(**{"x-litegen-conversion-losses": "uvs"}))
    respx.get(FORMATS_URL).mock(return_value=Response(200, json={"formats": ["glb", "obj"], "max_input_bytes": 10}))
    async with AsyncLiteGenClient() as client:
        out = await client.models3d.convert(GLB_BYTES, from_format="glb", to_format="obj")
        caps = await client.models3d.convert_formats()
    assert route.calls[0].request.content == GLB_BYTES
    assert out["losses"] == ["uvs"] and out["filename"] == "model.obj"
    assert caps["formats"] == ["glb", "obj"]
