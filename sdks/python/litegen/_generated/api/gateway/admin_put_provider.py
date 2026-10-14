from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.provider_config_view import ProviderConfigView
from ...models.put_provider_config_request import PutProviderConfigRequest
from ...types import Response


def _get_kwargs(
    name: str,
    *,
    body: PutProviderConfigRequest,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "put",
        "url": "/v1/admin/providers/{name}".format(
            name=name,
        ),
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, ProviderConfigView]]:
    if response.status_code == 200:
        response_200 = ProviderConfigView.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, ProviderConfigView]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    name: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutProviderConfigRequest,
) -> Response[Union[ErrorResponse, ProviderConfigView]]:
    """PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
    mapping, headers, options or enabled flag at runtime. Secret fields left
    out keep their stored value; every unset field falls back to env/YAML.
    In-flight jobs finish on the instance they started with.

    Args:
        name (str):
        body (PutProviderConfigRequest): Request body for `PUT /v1/admin/providers/{name}`. Secret
            fields left out
            keep their stored (or env) value, so an api_base change needs no key.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, ProviderConfigView]]
    """

    kwargs = _get_kwargs(
        name=name,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    name: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutProviderConfigRequest,
) -> Optional[Union[ErrorResponse, ProviderConfigView]]:
    """PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
    mapping, headers, options or enabled flag at runtime. Secret fields left
    out keep their stored value; every unset field falls back to env/YAML.
    In-flight jobs finish on the instance they started with.

    Args:
        name (str):
        body (PutProviderConfigRequest): Request body for `PUT /v1/admin/providers/{name}`. Secret
            fields left out
            keep their stored (or env) value, so an api_base change needs no key.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, ProviderConfigView]
    """

    return sync_detailed(
        name=name,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    name: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutProviderConfigRequest,
) -> Response[Union[ErrorResponse, ProviderConfigView]]:
    """PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
    mapping, headers, options or enabled flag at runtime. Secret fields left
    out keep their stored value; every unset field falls back to env/YAML.
    In-flight jobs finish on the instance they started with.

    Args:
        name (str):
        body (PutProviderConfigRequest): Request body for `PUT /v1/admin/providers/{name}`. Secret
            fields left out
            keep their stored (or env) value, so an api_base change needs no key.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, ProviderConfigView]]
    """

    kwargs = _get_kwargs(
        name=name,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    name: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: PutProviderConfigRequest,
) -> Optional[Union[ErrorResponse, ProviderConfigView]]:
    """PUT /v1/admin/providers/{name} — Set a provider's keys, api_base, model
    mapping, headers, options or enabled flag at runtime. Secret fields left
    out keep their stored value; every unset field falls back to env/YAML.
    In-flight jobs finish on the instance they started with.

    Args:
        name (str):
        body (PutProviderConfigRequest): Request body for `PUT /v1/admin/providers/{name}`. Secret
            fields left out
            keep their stored (or env) value, so an api_base change needs no key.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, ProviderConfigView]
    """

    return (
        await asyncio_detailed(
            name=name,
            client=client,
            body=body,
        )
    ).parsed
