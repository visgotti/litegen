from http import HTTPStatus
from typing import Any, Dict, Optional, Union, cast

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.org_transfer_owner_request import OrgTransferOwnerRequest
from ...types import Response


def _get_kwargs(
    id: str,
    *,
    body: OrgTransferOwnerRequest,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/orgs/{id}/transfer-owner".format(
            id=id,
        ),
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[Any, ErrorResponse]]:
    if response.status_code == 204:
        response_204 = cast(Any, None)
        return response_204
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
) -> Response[Union[Any, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: OrgTransferOwnerRequest,
) -> Response[Union[Any, ErrorResponse]]:
    """POST /v1/orgs/{id}/transfer-owner — Transfer ownership (requires org:transfer_owner).

    Args:
        id (str):
        body (OrgTransferOwnerRequest): Renamed in the OpenAPI schema to `OrgTransferOwnerRequest`
            to avoid a
            component name collision with `users::TransferOwnerRequest`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Any, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: OrgTransferOwnerRequest,
) -> Optional[Union[Any, ErrorResponse]]:
    """POST /v1/orgs/{id}/transfer-owner — Transfer ownership (requires org:transfer_owner).

    Args:
        id (str):
        body (OrgTransferOwnerRequest): Renamed in the OpenAPI schema to `OrgTransferOwnerRequest`
            to avoid a
            component name collision with `users::TransferOwnerRequest`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Any, ErrorResponse]
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: OrgTransferOwnerRequest,
) -> Response[Union[Any, ErrorResponse]]:
    """POST /v1/orgs/{id}/transfer-owner — Transfer ownership (requires org:transfer_owner).

    Args:
        id (str):
        body (OrgTransferOwnerRequest): Renamed in the OpenAPI schema to `OrgTransferOwnerRequest`
            to avoid a
            component name collision with `users::TransferOwnerRequest`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[Any, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    body: OrgTransferOwnerRequest,
) -> Optional[Union[Any, ErrorResponse]]:
    """POST /v1/orgs/{id}/transfer-owner — Transfer ownership (requires org:transfer_owner).

    Args:
        id (str):
        body (OrgTransferOwnerRequest): Renamed in the OpenAPI schema to `OrgTransferOwnerRequest`
            to avoid a
            component name collision with `users::TransferOwnerRequest`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[Any, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
