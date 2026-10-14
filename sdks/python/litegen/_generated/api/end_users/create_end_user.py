from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.create_end_user_request import CreateEndUserRequest
from ...models.end_user_view import EndUserView
from ...models.error_response import ErrorResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    body: CreateEndUserRequest,
    org_id: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    params: Dict[str, Any] = {}

    json_org_id: Union[None, Unset, str]
    if isinstance(org_id, Unset):
        json_org_id = UNSET
    else:
        json_org_id = org_id
    params["org_id"] = json_org_id

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/end-users",
        "params": params,
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[EndUserView, ErrorResponse]]:
    if response.status_code == 201:
        response_201 = EndUserView.from_dict(response.json())

        return response_201
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[EndUserView, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: CreateEndUserRequest,
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[EndUserView, ErrorResponse]]:
    """POST /v1/end-users — Register an end user ahead of its first request,
    optionally with limits.

    Args:
        org_id (Union[None, Unset, str]):
        body (CreateEndUserRequest): Request body for `POST /v1/end-users`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[EndUserView, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        org_id=org_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: CreateEndUserRequest,
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[EndUserView, ErrorResponse]]:
    """POST /v1/end-users — Register an end user ahead of its first request,
    optionally with limits.

    Args:
        org_id (Union[None, Unset, str]):
        body (CreateEndUserRequest): Request body for `POST /v1/end-users`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[EndUserView, ErrorResponse]
    """

    return sync_detailed(
        client=client,
        body=body,
        org_id=org_id,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: CreateEndUserRequest,
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[EndUserView, ErrorResponse]]:
    """POST /v1/end-users — Register an end user ahead of its first request,
    optionally with limits.

    Args:
        org_id (Union[None, Unset, str]):
        body (CreateEndUserRequest): Request body for `POST /v1/end-users`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[EndUserView, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        org_id=org_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: CreateEndUserRequest,
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[EndUserView, ErrorResponse]]:
    """POST /v1/end-users — Register an end user ahead of its first request,
    optionally with limits.

    Args:
        org_id (Union[None, Unset, str]):
        body (CreateEndUserRequest): Request body for `POST /v1/end-users`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[EndUserView, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
            org_id=org_id,
        )
    ).parsed
