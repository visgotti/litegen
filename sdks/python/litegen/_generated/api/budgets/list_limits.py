from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.limits_list_response import LimitsListResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    entity_type: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
    params: Dict[str, Any] = {}

    json_entity_type: Union[None, Unset, str]
    if isinstance(entity_type, Unset):
        json_entity_type = UNSET
    else:
        json_entity_type = entity_type
    params["entity_type"] = json_entity_type

    json_org_id: Union[None, Unset, str]
    if isinstance(org_id, Unset):
        json_org_id = UNSET
    else:
        json_org_id = org_id
    params["org_id"] = json_org_id

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/limits",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, LimitsListResponse]]:
    if response.status_code == 200:
        response_200 = LimitsListResponse.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 403:
        response_403 = ErrorResponse.from_dict(response.json())

        return response_403
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, LimitsListResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    entity_type: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, LimitsListResponse]]:
    """GET /v1/limits — Every limits row in the caller's organization.

    Args:
        entity_type (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, LimitsListResponse]]
    """

    kwargs = _get_kwargs(
        entity_type=entity_type,
        org_id=org_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    entity_type: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, LimitsListResponse]]:
    """GET /v1/limits — Every limits row in the caller's organization.

    Args:
        entity_type (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, LimitsListResponse]
    """

    return sync_detailed(
        client=client,
        entity_type=entity_type,
        org_id=org_id,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    entity_type: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, LimitsListResponse]]:
    """GET /v1/limits — Every limits row in the caller's organization.

    Args:
        entity_type (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, LimitsListResponse]]
    """

    kwargs = _get_kwargs(
        entity_type=entity_type,
        org_id=org_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    entity_type: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, LimitsListResponse]]:
    """GET /v1/limits — Every limits row in the caller's organization.

    Args:
        entity_type (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, LimitsListResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            entity_type=entity_type,
            org_id=org_id,
        )
    ).parsed
