from http import HTTPStatus
from typing import Any, Dict, List, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.provider_catalog_entry import ProviderCatalogEntry
from ...types import Response


def _get_kwargs() -> Dict[str, Any]:
    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/providers",
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[List["ProviderCatalogEntry"]]:
    if response.status_code == 200:
        response_200 = []
        _response_200 = response.json()
        for response_200_item_data in _response_200:
            response_200_item = ProviderCatalogEntry.from_dict(response_200_item_data)

            response_200.append(response_200_item)

        return response_200
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[List["ProviderCatalogEntry"]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[List["ProviderCatalogEntry"]]:
    r"""GET /v1/providers — Catalog of supported providers and the credential fields
    each one needs. Drives the dashboard's dynamic credential form.
    Live: `curl https://app.litegen.ai/api/v1/providers -H \"Authorization: Bearer sk_live_...\"`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[List['ProviderCatalogEntry']]
    """

    kwargs = _get_kwargs()

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[List["ProviderCatalogEntry"]]:
    r"""GET /v1/providers — Catalog of supported providers and the credential fields
    each one needs. Drives the dashboard's dynamic credential form.
    Live: `curl https://app.litegen.ai/api/v1/providers -H \"Authorization: Bearer sk_live_...\"`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        List['ProviderCatalogEntry']
    """

    return sync_detailed(
        client=client,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Response[List["ProviderCatalogEntry"]]:
    r"""GET /v1/providers — Catalog of supported providers and the credential fields
    each one needs. Drives the dashboard's dynamic credential form.
    Live: `curl https://app.litegen.ai/api/v1/providers -H \"Authorization: Bearer sk_live_...\"`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[List['ProviderCatalogEntry']]
    """

    kwargs = _get_kwargs()

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
) -> Optional[List["ProviderCatalogEntry"]]:
    r"""GET /v1/providers — Catalog of supported providers and the credential fields
    each one needs. Drives the dashboard's dynamic credential form.
    Live: `curl https://app.litegen.ai/api/v1/providers -H \"Authorization: Bearer sk_live_...\"`

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        List['ProviderCatalogEntry']
    """

    return (
        await asyncio_detailed(
            client=client,
        )
    ).parsed
