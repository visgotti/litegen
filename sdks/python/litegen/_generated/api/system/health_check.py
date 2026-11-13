from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.health_response import HealthResponse
from ...types import Response


def _get_kwargs() -> Dict[str, Any]:
    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/health",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, HealthResponse]]:
    if response.status_code == 200:
        response_200 = HealthResponse.from_dict(response.json())

        return response_200
    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, HealthResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, HealthResponse]]:
    """GET /health — Health check for all providers.

     Checks each provider configured with the gateway's own keys, side by
    side, and reports what each vendor answered. It reads the deployment's
    own state, so it takes the master key, or, in single-tenant mode, the
    `read` scope in the deployment's org.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, HealthResponse]]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, HealthResponse]]:
    """GET /health — Health check for all providers.

     Checks each provider configured with the gateway's own keys, side by
    side, and reports what each vendor answered. It reads the deployment's
    own state, so it takes the master key, or, in single-tenant mode, the
    `read` scope in the deployment's org.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, HealthResponse]
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, HealthResponse]]:
    """GET /health — Health check for all providers.

     Checks each provider configured with the gateway's own keys, side by
    side, and reports what each vendor answered. It reads the deployment's
    own state, so it takes the master key, or, in single-tenant mode, the
    `read` scope in the deployment's org.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, HealthResponse]]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, HealthResponse]]:
    """GET /health — Health check for all providers.

     Checks each provider configured with the gateway's own keys, side by
    side, and reports what each vendor answered. It reads the deployment's
    own state, so it takes the master key, or, in single-tenant mode, the
    `read` scope in the deployment's org.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, HealthResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
