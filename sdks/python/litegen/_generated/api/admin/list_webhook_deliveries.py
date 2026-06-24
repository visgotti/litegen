from http import HTTPStatus
from typing import Any, Dict, Optional, Union
from uuid import UUID

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.paginated_response_webhook_delivery import (
    PaginatedResponseWebhookDelivery,
)
from ...types import UNSET, Response, Unset


def _get_kwargs(
    id: UUID,
    *,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Dict[str, Any]:
    params: Dict[str, Any] = {}

    params["page"] = page

    params["per_page"] = per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/keys/{id}/webhook-deliveries".format(
            id=id,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
    if response.status_code == 200:
        response_200 = PaginatedResponseWebhookDelivery.from_dict(response.json())

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
) -> Response[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
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
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
    r"""GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
    Live: `curl https://app.litegen.ai/api/v1/keys/{id}/webhook-deliveries -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]
    """

    kwargs = _get_kwargs(
        id=id,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
    r"""GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
    Live: `curl https://app.litegen.ai/api/v1/keys/{id}/webhook-deliveries -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseWebhookDelivery]
    """

    return sync_detailed(
        id=id,
        client=client,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
    r"""GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
    Live: `curl https://app.litegen.ai/api/v1/keys/{id}/webhook-deliveries -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]
    """

    kwargs = _get_kwargs(
        id=id,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseWebhookDelivery]]:
    r"""GET /v1/keys/{id}/webhook-deliveries — List webhook deliveries for a key (admin scope).
    Live: `curl https://app.litegen.ai/api/v1/keys/{id}/webhook-deliveries -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        id (UUID):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseWebhookDelivery]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            page=page,
            per_page=per_page,
        )
    ).parsed
