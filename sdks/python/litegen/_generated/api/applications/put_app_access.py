from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.app_access_settings import AppAccessSettings
from ...models.error_response import ErrorResponse
from ...models.put_app_access_request import PutAppAccessRequest
from ...types import Response


def _get_kwargs(
    app_id: str,
    *,
    body: PutAppAccessRequest,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "put",
        "url": "/v1/apps/{app_id}/access".format(
            app_id=app_id,
        ),
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AppAccessSettings, ErrorResponse]]:
    if response.status_code == 200:
        response_200 = AppAccessSettings.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[AppAccessSettings, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    app_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutAppAccessRequest,
) -> Response[Union[AppAccessSettings, ErrorResponse]]:
    """PUT /v1/apps/{app_id}/access — replace the app's stored-file access
    settings. An omitted or `null` field uses the gateway's value.

    Args:
        app_id (str):
        body (PutAppAccessRequest): `PUT /v1/apps/{app_id}/access`: the app's settings, every one
            replaced.
            An omitted or `null` field uses the gateway's value; `{}` clears them all.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppAccessSettings, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        app_id=app_id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    app_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutAppAccessRequest,
) -> Optional[Union[AppAccessSettings, ErrorResponse]]:
    """PUT /v1/apps/{app_id}/access — replace the app's stored-file access
    settings. An omitted or `null` field uses the gateway's value.

    Args:
        app_id (str):
        body (PutAppAccessRequest): `PUT /v1/apps/{app_id}/access`: the app's settings, every one
            replaced.
            An omitted or `null` field uses the gateway's value; `{}` clears them all.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppAccessSettings, ErrorResponse]
    """

    return sync_detailed(
        app_id=app_id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    app_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutAppAccessRequest,
) -> Response[Union[AppAccessSettings, ErrorResponse]]:
    """PUT /v1/apps/{app_id}/access — replace the app's stored-file access
    settings. An omitted or `null` field uses the gateway's value.

    Args:
        app_id (str):
        body (PutAppAccessRequest): `PUT /v1/apps/{app_id}/access`: the app's settings, every one
            replaced.
            An omitted or `null` field uses the gateway's value; `{}` clears them all.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AppAccessSettings, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        app_id=app_id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    app_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutAppAccessRequest,
) -> Optional[Union[AppAccessSettings, ErrorResponse]]:
    """PUT /v1/apps/{app_id}/access — replace the app's stored-file access
    settings. An omitted or `null` field uses the gateway's value.

    Args:
        app_id (str):
        body (PutAppAccessRequest): `PUT /v1/apps/{app_id}/access`: the app's settings, every one
            replaced.
            An omitted or `null` field uses the gateway's value; `{}` clears them all.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AppAccessSettings, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            app_id=app_id,
            client=client,
            body=body,
        )
    ).parsed
