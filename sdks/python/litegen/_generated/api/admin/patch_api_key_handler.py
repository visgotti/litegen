from http import HTTPStatus
from typing import Any, Dict, Optional, Union
from uuid import UUID

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.api_key_detail import ApiKeyDetail
from ...models.error_response import ErrorResponse
from ...models.update_api_key_request import UpdateApiKeyRequest
from ...types import Response


def _get_kwargs(
    id: UUID,
    *,
    body: UpdateApiKeyRequest,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "patch",
        "url": "/v1/keys/{id}".format(
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
) -> Optional[Union[ApiKeyDetail, ErrorResponse]]:
    if response.status_code == 200:
        response_200 = ApiKeyDetail.from_dict(response.json())

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
) -> Response[Union[ApiKeyDetail, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    body: UpdateApiKeyRequest,
) -> Response[Union[ApiKeyDetail, ErrorResponse]]:
    r"""PATCH /v1/keys/:id — Update an API key's quota/rpm/scopes/etc.
    Live: `curl -X PATCH https://app.litegen.ai/api/v1/keys/{id} -H \"Authorization: Bearer
    sk_live_...\" -d '{\"rpm_limit\":120}'`

    Args:
        id (UUID):
        body (UpdateApiKeyRequest): Request body for PATCH /v1/keys/{id}.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ApiKeyDetail, ErrorResponse]]
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
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    body: UpdateApiKeyRequest,
) -> Optional[Union[ApiKeyDetail, ErrorResponse]]:
    r"""PATCH /v1/keys/:id — Update an API key's quota/rpm/scopes/etc.
    Live: `curl -X PATCH https://app.litegen.ai/api/v1/keys/{id} -H \"Authorization: Bearer
    sk_live_...\" -d '{\"rpm_limit\":120}'`

    Args:
        id (UUID):
        body (UpdateApiKeyRequest): Request body for PATCH /v1/keys/{id}.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ApiKeyDetail, ErrorResponse]
    """

    return sync_detailed(
        id=id,
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    body: UpdateApiKeyRequest,
) -> Response[Union[ApiKeyDetail, ErrorResponse]]:
    r"""PATCH /v1/keys/:id — Update an API key's quota/rpm/scopes/etc.
    Live: `curl -X PATCH https://app.litegen.ai/api/v1/keys/{id} -H \"Authorization: Bearer
    sk_live_...\" -d '{\"rpm_limit\":120}'`

    Args:
        id (UUID):
        body (UpdateApiKeyRequest): Request body for PATCH /v1/keys/{id}.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ApiKeyDetail, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        id=id,
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    id: UUID,
    *,
    client: Union[AuthenticatedClient, Client],
    body: UpdateApiKeyRequest,
) -> Optional[Union[ApiKeyDetail, ErrorResponse]]:
    r"""PATCH /v1/keys/:id — Update an API key's quota/rpm/scopes/etc.
    Live: `curl -X PATCH https://app.litegen.ai/api/v1/keys/{id} -H \"Authorization: Bearer
    sk_live_...\" -d '{\"rpm_limit\":120}'`

    Args:
        id (UUID):
        body (UpdateApiKeyRequest): Request body for PATCH /v1/keys/{id}.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ApiKeyDetail, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            id=id,
            client=client,
            body=body,
        )
    ).parsed
