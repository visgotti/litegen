from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.image_segmentation_request import ImageSegmentationRequest
from ...models.image_segmentation_response import ImageSegmentationResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    body: ImageSegmentationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}
    if not isinstance(idempotency_key, Unset):
        headers["Idempotency-Key"] = idempotency_key

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/images/segmentations",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, ImageSegmentationResponse]]:
    if response.status_code == 200:
        response_200 = ImageSegmentationResponse.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 402:
        response_402 = ErrorResponse.from_dict(response.json())

        return response_402
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409
    if response.status_code == 410:
        response_410 = ErrorResponse.from_dict(response.json())

        return response_410
    if response.status_code == 429:
        response_429 = ErrorResponse.from_dict(response.json())

        return response_429
    if response.status_code == 502:
        response_502 = ErrorResponse.from_dict(response.json())

        return response_502
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
) -> Response[Union[ErrorResponse, ImageSegmentationResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ImageSegmentationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, ImageSegmentationResponse]]:
    """POST /v1/images/segmentations — Masks for what point, box or text prompts
    select in an image, answered at once.

    Args:
        idempotency_key (Union[None, Unset, str]):
        body (ImageSegmentationRequest): `POST /v1/images/segmentations` (PixelGenie L7). Answered
            at once.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, ImageSegmentationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ImageSegmentationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, ImageSegmentationResponse]]:
    """POST /v1/images/segmentations — Masks for what point, box or text prompts
    select in an image, answered at once.

    Args:
        idempotency_key (Union[None, Unset, str]):
        body (ImageSegmentationRequest): `POST /v1/images/segmentations` (PixelGenie L7). Answered
            at once.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, ImageSegmentationResponse]
    """

    return sync_detailed(
        client=client,
        body=body,
        idempotency_key=idempotency_key,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ImageSegmentationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, ImageSegmentationResponse]]:
    """POST /v1/images/segmentations — Masks for what point, box or text prompts
    select in an image, answered at once.

    Args:
        idempotency_key (Union[None, Unset, str]):
        body (ImageSegmentationRequest): `POST /v1/images/segmentations` (PixelGenie L7). Answered
            at once.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, ImageSegmentationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ImageSegmentationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, ImageSegmentationResponse]]:
    """POST /v1/images/segmentations — Masks for what point, box or text prompts
    select in an image, answered at once.

    Args:
        idempotency_key (Union[None, Unset, str]):
        body (ImageSegmentationRequest): `POST /v1/images/segmentations` (PixelGenie L7). Answered
            at once.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, ImageSegmentationResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
            idempotency_key=idempotency_key,
        )
    ).parsed
