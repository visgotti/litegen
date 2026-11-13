from http import HTTPStatus
from typing import Any, Dict, Optional, Union, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...types import UNSET, Response


def _get_kwargs(
    id: str,
    index: int,
    *,
    expires: int,
    issued: int,
    signature: str,
) -> Dict[str, Any]:
    params: Dict[str, Any] = {}

    params["expires"] = expires

    params["issued"] = issued

    params["signature"] = signature

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/generations/{id}/assets/{index}/signed".format(
            id=id,
            index=index,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Any, ErrorResponse]]:
    if response.status_code == 200:
        response_200 = cast(Any, None)
        return response_200
    if response.status_code == 206:
        response_206 = cast(Any, None)
        return response_206
    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if response.status_code == 410:
        response_410 = ErrorResponse.from_dict(response.json())

        return response_410
    if response.status_code == 416:
        response_416 = ErrorResponse.from_dict(response.json())

        return response_416
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[Any, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    index: int,
    *,
    client: Union[AuthenticatedClient, Client],
    expires: int,
    issued: int,
    signature: str,
) -> Response[Union[Any, ErrorResponse]]:
    """GET /v1/generations/{id}/assets/{index}/signed — One file, through a link litegen signed.

    Args:
        id (str):
        index (int):
        expires (int):
        issued (int):
        signature (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Any, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
        index=index,
        expires=expires,
        issued=issued,
        signature=signature,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    index: int,
    *,
    client: Union[AuthenticatedClient, Client],
    expires: int,
    issued: int,
    signature: str,
) -> Optional[Union[Any, ErrorResponse]]:
    """GET /v1/generations/{id}/assets/{index}/signed — One file, through a link litegen signed.

    Args:
        id (str):
        index (int):
        expires (int):
        issued (int):
        signature (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Any, ErrorResponse]
    """

    return sync_detailed(
        id=id,
        index=index,
        client=client,
        expires=expires,
        issued=issued,
        signature=signature,
    ).parsed


async def asyncio_detailed(
    id: str,
    index: int,
    *,
    client: Union[AuthenticatedClient, Client],
    expires: int,
    issued: int,
    signature: str,
) -> Response[Union[Any, ErrorResponse]]:
    """GET /v1/generations/{id}/assets/{index}/signed — One file, through a link litegen signed.

    Args:
        id (str):
        index (int):
        expires (int):
        issued (int):
        signature (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Any, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
        index=index,
        expires=expires,
        issued=issued,
        signature=signature,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    index: int,
    *,
    client: Union[AuthenticatedClient, Client],
    expires: int,
    issued: int,
    signature: str,
) -> Optional[Union[Any, ErrorResponse]]:
    """GET /v1/generations/{id}/assets/{index}/signed — One file, through a link litegen signed.

    Args:
        id (str):
        index (int):
        expires (int):
        issued (int):
        signature (str):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Any, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            id=id,
            index=index,
            client=client,
            expires=expires,
            issued=issued,
            signature=signature,
        )
    ).parsed
