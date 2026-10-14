from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.gateway_settings import GatewaySettings
from ...models.gateway_settings_view import GatewaySettingsView
from ...types import Response


def _get_kwargs(
    *,
    body: GatewaySettings,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "put",
        "url": "/v1/admin/settings",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, GatewaySettingsView]]:
    if response.status_code == 200:
        response_200 = GatewaySettingsView.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, GatewaySettingsView]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: GatewaySettings,
) -> Response[Union[ErrorResponse, GatewaySettingsView]]:
    """PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
    webhook, default end-user budget). An omitted field is cleared.

    Args:
        body (GatewaySettings): Gateway-wide settings editable at runtime.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, GatewaySettingsView]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: GatewaySettings,
) -> Optional[Union[ErrorResponse, GatewaySettingsView]]:
    """PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
    webhook, default end-user budget). An omitted field is cleared.

    Args:
        body (GatewaySettings): Gateway-wide settings editable at runtime.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, GatewaySettingsView]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: GatewaySettings,
) -> Response[Union[ErrorResponse, GatewaySettingsView]]:
    """PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
    webhook, default end-user budget). An omitted field is cleared.

    Args:
        body (GatewaySettings): Gateway-wide settings editable at runtime.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, GatewaySettingsView]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: GatewaySettings,
) -> Optional[Union[ErrorResponse, GatewaySettingsView]]:
    """PUT /v1/admin/settings — Replace the gateway settings (markup, budget alert
    webhook, default end-user budget). An omitted field is cleared.

    Args:
        body (GatewaySettings): Gateway-wide settings editable at runtime.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, GatewaySettingsView]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
