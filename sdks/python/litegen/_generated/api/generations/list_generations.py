from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.paginated_response_generation import PaginatedResponseGeneration
from ...types import UNSET, Response, Unset


def _get_kwargs(
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
        "url": "/v1/generations",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, PaginatedResponseGeneration]]:
    if response.status_code == 200:
        response_200 = PaginatedResponseGeneration.from_dict(response.json())

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
) -> Response[Union[ErrorResponse, PaginatedResponseGeneration]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseGeneration]]:
    r"""GET /v1/generations — Paginated list of generations.
    Live: `curl https://app.litegen.ai/api/v1/generations?page=1&per_page=50 -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseGeneration]]
    """

    kwargs = _get_kwargs(
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseGeneration]]:
    r"""GET /v1/generations — Paginated list of generations.
    Live: `curl https://app.litegen.ai/api/v1/generations?page=1&per_page=50 -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseGeneration]
    """

    return sync_detailed(
        client=client,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseGeneration]]:
    r"""GET /v1/generations — Paginated list of generations.
    Live: `curl https://app.litegen.ai/api/v1/generations?page=1&per_page=50 -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseGeneration]]
    """

    kwargs = _get_kwargs(
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseGeneration]]:
    r"""GET /v1/generations — Paginated list of generations.
    Live: `curl https://app.litegen.ai/api/v1/generations?page=1&per_page=50 -H \"Authorization: Bearer
    sk_live_...\"`

    Args:
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseGeneration]
    """

    return (
        await asyncio_detailed(
            client=client,
            page=page,
            per_page=per_page,
        )
    ).parsed
