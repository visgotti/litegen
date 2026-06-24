from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.readiness_response import ReadinessResponse
from ...types import Response


def _get_kwargs() -> Dict[str, Any]:
    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/health/ready",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[ReadinessResponse]:
    if response.status_code == 200:
        response_200 = ReadinessResponse.from_dict(response.json())

        return response_200
    if response.status_code == 503:
        response_503 = ReadinessResponse.from_dict(response.json())

        return response_503
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[ReadinessResponse]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[ReadinessResponse]:
    """GET /health/ready — Readiness probe.
    Returns 200 when the DB is reachable (the instance can accept requests).
    Provider availability is resolved per-request — globally configured creds OR
    org-scoped BYO credentials — so it does NOT gate readiness; the healthy
    provider list is reported in the body purely as an informational signal.
    Returns 503 only when the DB is unreachable. No auth required.
    Live: `curl https://app.litegen.ai/api/health/ready`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ReadinessResponse]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[ReadinessResponse]:
    """GET /health/ready — Readiness probe.
    Returns 200 when the DB is reachable (the instance can accept requests).
    Provider availability is resolved per-request — globally configured creds OR
    org-scoped BYO credentials — so it does NOT gate readiness; the healthy
    provider list is reported in the body purely as an informational signal.
    Returns 503 only when the DB is unreachable. No auth required.
    Live: `curl https://app.litegen.ai/api/health/ready`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ReadinessResponse
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[ReadinessResponse]:
    """GET /health/ready — Readiness probe.
    Returns 200 when the DB is reachable (the instance can accept requests).
    Provider availability is resolved per-request — globally configured creds OR
    org-scoped BYO credentials — so it does NOT gate readiness; the healthy
    provider list is reported in the body purely as an informational signal.
    Returns 503 only when the DB is unreachable. No auth required.
    Live: `curl https://app.litegen.ai/api/health/ready`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[ReadinessResponse]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[ReadinessResponse]:
    """GET /health/ready — Readiness probe.
    Returns 200 when the DB is reachable (the instance can accept requests).
    Provider availability is resolved per-request — globally configured creds OR
    org-scoped BYO credentials — so it does NOT gate readiness; the healthy
    provider list is reported in the body purely as an informational signal.
    Returns 503 only when the DB is unreachable. No auth required.
    Live: `curl https://app.litegen.ai/api/health/ready`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        ReadinessResponse
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
