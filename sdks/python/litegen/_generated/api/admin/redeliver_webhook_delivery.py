from http import HTTPStatus
from typing import Any, Dict, Optional, Union
from uuid import UUID

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.webhook_delivery_state import WebhookDeliveryState
from ...types import Response


def _get_kwargs(
    id: UUID,
    delivery_id: str,
) -> Dict[str, Any]:
    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver".format(
            id=id,
            delivery_id=delivery_id,
        ),
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, WebhookDeliveryState]]:
    if response.status_code == 202:
        response_202 = WebhookDeliveryState.from_dict(response.json())

        return response_202
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409
    if response.status_code == 429:
        response_429 = ErrorResponse.from_dict(response.json())

        return response_429
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, WebhookDeliveryState]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: UUID,
    delivery_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, WebhookDeliveryState]]:
    r"""POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver — Send one of the key's webhook
    deliveries again.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/webhook-
    deliveries/{delivery_id}/redeliver -H \"Authorization: Bearer sk_live_...\"`

    Args:
        id (UUID):
        delivery_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, WebhookDeliveryState]]
    """

    kwargs = _get_kwargs(
        id=id,
        delivery_id=delivery_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: UUID,
    delivery_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, WebhookDeliveryState]]:
    r"""POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver — Send one of the key's webhook
    deliveries again.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/webhook-
    deliveries/{delivery_id}/redeliver -H \"Authorization: Bearer sk_live_...\"`

    Args:
        id (UUID):
        delivery_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, WebhookDeliveryState]
    """

    return sync_detailed(
        id=id,
        delivery_id=delivery_id,
        client=client,
    ).parsed


async def asyncio_detailed(
    id: UUID,
    delivery_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[Union[ErrorResponse, WebhookDeliveryState]]:
    r"""POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver — Send one of the key's webhook
    deliveries again.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/webhook-
    deliveries/{delivery_id}/redeliver -H \"Authorization: Bearer sk_live_...\"`

    Args:
        id (UUID):
        delivery_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, WebhookDeliveryState]]
    """

    kwargs = _get_kwargs(
        id=id,
        delivery_id=delivery_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: UUID,
    delivery_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[Union[ErrorResponse, WebhookDeliveryState]]:
    r"""POST /v1/keys/{id}/webhook-deliveries/{delivery_id}/redeliver — Send one of the key's webhook
    deliveries again.
    Live: `curl -X POST https://app.litegen.ai/api/v1/keys/{id}/webhook-
    deliveries/{delivery_id}/redeliver -H \"Authorization: Bearer sk_live_...\"`

    Args:
        id (UUID):
        delivery_id (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, WebhookDeliveryState]
    """

    return (
        await asyncio_detailed(
            id=id,
            delivery_id=delivery_id,
            client=client,
        )
    ).parsed
