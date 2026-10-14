from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.admin_model_view import AdminModelView
from ...models.error_response import ErrorResponse
from ...models.model_schema import ModelSchema
from ...types import Response


def _get_kwargs(
    *,
    body: ModelSchema,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/admin/models",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    if response.status_code == 201:
        response_201 = AdminModelView.from_dict(response.json())

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
) -> Response[Union[AdminModelView, ErrorResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ModelSchema,
) -> Response[Union[AdminModelView, ErrorResponse]]:
    """POST /v1/admin/models — Add a model at runtime (LiteLLM `/model/new`).

     The body is a full model schema, the same shape as a `models/*.yaml` row,
    and passes the same validation. The vendor model called is the part of the
    id after `<provider>/` (or the provider's `model_mapping`).

    Args:
        body (ModelSchema):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AdminModelView, ErrorResponse]]
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
    body: ModelSchema,
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    """POST /v1/admin/models — Add a model at runtime (LiteLLM `/model/new`).

     The body is a full model schema, the same shape as a `models/*.yaml` row,
    and passes the same validation. The vendor model called is the part of the
    id after `<provider>/` (or the provider's `model_mapping`).

    Args:
        body (ModelSchema):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AdminModelView, ErrorResponse]
    """

    return sync_detailed(
        client=client,
        body=body,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ModelSchema,
) -> Response[Union[AdminModelView, ErrorResponse]]:
    """POST /v1/admin/models — Add a model at runtime (LiteLLM `/model/new`).

     The body is a full model schema, the same shape as a `models/*.yaml` row,
    and passes the same validation. The vendor model called is the part of the
    id after `<provider>/` (or the provider's `model_mapping`).

    Args:
        body (ModelSchema):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[AdminModelView, ErrorResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: ModelSchema,
) -> Optional[Union[AdminModelView, ErrorResponse]]:
    """POST /v1/admin/models — Add a model at runtime (LiteLLM `/model/new`).

     The body is a full model schema, the same shape as a `models/*.yaml` row,
    and passes the same validation. The vendor model called is the part of the
    id after `<provider>/` (or the provider's `model_mapping`).

    Args:
        body (ModelSchema):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[AdminModelView, ErrorResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
        )
    ).parsed
