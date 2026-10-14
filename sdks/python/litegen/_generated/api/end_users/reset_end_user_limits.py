from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.entity_limits import EntityLimits
from ...models.error_response import ErrorResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    end_user_id: str,
    *,
    org_id: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
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
        "url": "/v1/end-users/{end_user_id}/limits/reset".format(
            end_user_id=end_user_id,
        ),
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[EntityLimits, ErrorResponse]]:
    if response.status_code == 200:
        response_200 = EntityLimits.from_dict(response.json())

        return response_200
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[EntityLimits, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    end_user_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[EntityLimits, ErrorResponse]]:
    """POST /v1/end-users/{end_user_id}/limits/reset

    Args:
        end_user_id (str):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[EntityLimits, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        end_user_id=end_user_id,
        org_id=org_id,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    end_user_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[EntityLimits, ErrorResponse]]:
    """POST /v1/end-users/{end_user_id}/limits/reset

    Args:
        end_user_id (str):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[EntityLimits, ErrorResponse]
    """

    return sync_detailed(
        end_user_id=end_user_id,
        client=client,
        org_id=org_id,
    ).parsed


async def asyncio_detailed(
    end_user_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    org_id: Union[None, Unset, str] = UNSET,
) -> Response[Union[EntityLimits, ErrorResponse]]:
    """POST /v1/end-users/{end_user_id}/limits/reset

    Args:
        end_user_id (str):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[EntityLimits, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        end_user_id=end_user_id,
        org_id=org_id,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    end_user_id: str,
    *,
    client: Union[AuthenticatedClient, Client],
    org_id: Union[None, Unset, str] = UNSET,
) -> Optional[Union[EntityLimits, ErrorResponse]]:
    """POST /v1/end-users/{end_user_id}/limits/reset

    Args:
        end_user_id (str):
        org_id (Union[None, Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[EntityLimits, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            end_user_id=end_user_id,
            client=client,
            org_id=org_id,
        )
    ).parsed
