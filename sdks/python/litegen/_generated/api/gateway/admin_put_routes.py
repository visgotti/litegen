from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.routes_document import RoutesDocument
from ...types import Response


def _get_kwargs(
    *,
    body: RoutesDocument,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "put",
        "url": "/v1/admin/routes",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, RoutesDocument]]:
    if response.status_code == 200:
        response_200 = RoutesDocument.from_dict(response.json())

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
) -> Response[Union[ErrorResponse, RoutesDocument]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RoutesDocument,
) -> Response[Union[ErrorResponse, RoutesDocument]]:
    """PUT /v1/admin/routes — Replace every model route at runtime. Order matters:
    the first route whose pattern matches a model wins.

    Args:
        body (RoutesDocument): Body and response for `GET`/`PUT /v1/admin/routes`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, RoutesDocument]]
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
    body: RoutesDocument,
) -> Optional[Union[ErrorResponse, RoutesDocument]]:
    """PUT /v1/admin/routes — Replace every model route at runtime. Order matters:
    the first route whose pattern matches a model wins.

    Args:
        body (RoutesDocument): Body and response for `GET`/`PUT /v1/admin/routes`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, RoutesDocument]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RoutesDocument,
) -> Response[Union[ErrorResponse, RoutesDocument]]:
    """PUT /v1/admin/routes — Replace every model route at runtime. Order matters:
    the first route whose pattern matches a model wins.

    Args:
        body (RoutesDocument): Body and response for `GET`/`PUT /v1/admin/routes`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, RoutesDocument]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: RoutesDocument,
) -> Optional[Union[ErrorResponse, RoutesDocument]]:
    """PUT /v1/admin/routes — Replace every model route at runtime. Order matters:
    the first route whose pattern matches a model wins.

    Args:
        body (RoutesDocument): Body and response for `GET`/`PUT /v1/admin/routes`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, RoutesDocument]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
