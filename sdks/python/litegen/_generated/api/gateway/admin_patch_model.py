from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.admin_model_view import AdminModelView
from ...models.error_response import ErrorResponse
from ...models.patch_model_state_request import PatchModelStateRequest
from ...types import Response


def _get_kwargs(
    id: str,
    *,
    body: PatchModelStateRequest,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "patch",
        "url": "/v1/admin/models/{id}".format(
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
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    if response.status_code == 200:
        response_200 = AdminModelView.from_dict(response.json())

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
) -> Response[Union[AdminModelView, ErrorResponse]]:
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
    body: PatchModelStateRequest,
) -> Response[Union[AdminModelView, ErrorResponse]]:
    """PATCH /v1/admin/models/{id} — Enable or disable any model platform-wide
    (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and
    generation requests for them answer 404 `model_not_found`.

    Args:
        id (str):
        body (PatchModelStateRequest): Request body for `PATCH /v1/admin/models/{id}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AdminModelView, ErrorResponse]]
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
    body: PatchModelStateRequest,
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    """PATCH /v1/admin/models/{id} — Enable or disable any model platform-wide
    (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and
    generation requests for them answer 404 `model_not_found`.

    Args:
        id (str):
        body (PatchModelStateRequest): Request body for `PATCH /v1/admin/models/{id}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AdminModelView, ErrorResponse]
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
    body: PatchModelStateRequest,
) -> Response[Union[AdminModelView, ErrorResponse]]:
    """PATCH /v1/admin/models/{id} — Enable or disable any model platform-wide
    (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and
    generation requests for them answer 404 `model_not_found`.

    Args:
        id (str):
        body (PatchModelStateRequest): Request body for `PATCH /v1/admin/models/{id}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AdminModelView, ErrorResponse]]
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
    body: PatchModelStateRequest,
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    """PATCH /v1/admin/models/{id} — Enable or disable any model platform-wide
    (LiteLLM `/model/block`). Disabled models disappear from `/v1/models` and
    generation requests for them answer 404 `model_not_found`.

    Args:
        id (str):
        body (PatchModelStateRequest): Request body for `PATCH /v1/admin/models/{id}`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AdminModelView, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
