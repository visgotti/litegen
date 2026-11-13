from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.video_generation_request import VideoGenerationRequest
from ...models.video_generation_response import VideoGenerationResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    body: VideoGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}
    if not isinstance(idempotency_key, Unset):
        headers["Idempotency-Key"] = idempotency_key

    if not isinstance(x_litegen_keep_outputs, Unset):
        headers["X-Litegen-Keep-Outputs"] = x_litegen_keep_outputs

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/videos/generations",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, VideoGenerationResponse]]:
    if response.status_code == 200:
        response_200 = VideoGenerationResponse.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if response.status_code == 408:
        response_408 = ErrorResponse.from_dict(response.json())

        return response_408
    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409
    if response.status_code == 410:
        response_410 = ErrorResponse.from_dict(response.json())

        return response_410
    if response.status_code == 413:
        response_413 = ErrorResponse.from_dict(response.json())

        return response_413
    if response.status_code == 422:
        response_422 = ErrorResponse.from_dict(response.json())

        return response_422
    if response.status_code == 500:
        response_500 = ErrorResponse.from_dict(response.json())

        return response_500
    if response.status_code == 503:
        response_503 = ErrorResponse.from_dict(response.json())

        return response_503
    if response.status_code == 504:
        response_504 = ErrorResponse.from_dict(response.json())

        return response_504
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, VideoGenerationResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: VideoGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, VideoGenerationResponse]]:
    """POST /v1/videos/generations — Start video generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (VideoGenerationRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, VideoGenerationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: VideoGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, VideoGenerationResponse]]:
    """POST /v1/videos/generations — Start video generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (VideoGenerationRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, VideoGenerationResponse]
    """

    return sync_detailed(
        client=client,
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: VideoGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, VideoGenerationResponse]]:
    """POST /v1/videos/generations — Start video generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (VideoGenerationRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, VideoGenerationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: VideoGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, VideoGenerationResponse]]:
    """POST /v1/videos/generations — Start video generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (VideoGenerationRequest):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, VideoGenerationResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
            idempotency_key=idempotency_key,
            x_litegen_keep_outputs=x_litegen_keep_outputs,
        )
    ).parsed
