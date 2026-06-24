from http import HTTPStatus
from typing import Any, Dict, Optional, Union
from uuid import UUID

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.rotated_key_response import RotatedKeyResponse
from ...types import Response


def _get_kwargs(
    id: UUID,
) -> Dict[str, Any]:
    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/keys/{id}/rotate".format(
            id=id,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, RotatedKeyResponse]]:
    if response.status_code == 200:
        response_200 = RotatedKeyResponse.from_dict(response.json())

        return response_200
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, RotatedKeyResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, RotatedKeyResponse]]:
    r"""POST /v1/keys/{id}/rotate — Issue a new secret for an existing key in place.
    Keeps the same row id and all settings; the new `sk_live_` secret is returned once.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/rotate -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, RotatedKeyResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, RotatedKeyResponse]]:
    r"""POST /v1/keys/{id}/rotate — Issue a new secret for an existing key in place.
    Keeps the same row id and all settings; the new `sk_live_` secret is returned once.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/rotate -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, RotatedKeyResponse]
    """

    return sync_detailed(
        id=id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, RotatedKeyResponse]]:
    r"""POST /v1/keys/{id}/rotate — Issue a new secret for an existing key in place.
    Keeps the same row id and all settings; the new `sk_live_` secret is returned once.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/rotate -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, RotatedKeyResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, RotatedKeyResponse]]:
    r"""POST /v1/keys/{id}/rotate — Issue a new secret for an existing key in place.
    Keeps the same row id and all settings; the new `sk_live_` secret is returned once.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/rotate -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, RotatedKeyResponse]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
        )
    ).parsed
