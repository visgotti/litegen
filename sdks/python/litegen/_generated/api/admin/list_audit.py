from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.paginated_response_audit_log_entry import PaginatedResponseAuditLogEntry
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    actor_key_id: Union[Unset, str] = UNSET,
    action: Union[Unset, str] = UNSET,
    from_: Union[Unset, str] = UNSET,
    to: Union[Unset, str] = UNSET,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
    format_: Union[Unset, str] = UNSET,
) -> Dict[str, Any]:
    params: Dict[str, Any] = {}

    params["actor_key_id"] = actor_key_id

    params["action"] = action

    params["from"] = from_

    params["to"] = to

    params["page"] = page

    params["per_page"] = per_page

    params["format"] = format_

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/audit",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    if response.status_code == 200:
        response_200 = PaginatedResponseAuditLogEntry.from_dict(response.json())

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
) -> Response[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    actor_key_id: Union[Unset, str] = UNSET,
    action: Union[Unset, str] = UNSET,
    from_: Union[Unset, str] = UNSET,
    to: Union[Unset, str] = UNSET,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
    format_: Union[Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    r"""GET /v1/audit — List audit log entries (admin scope required). Supports `?format=csv`.
    Live: `curl https://app.litegen.ai/api/v1/audit -H \"Authorization: Bearer sk_live_...\"`

    Args:
        actor_key_id (Union[Unset, str]):
        action (Union[Unset, str]):
        from_ (Union[Unset, str]):
        to (Union[Unset, str]):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):
        format_ (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]
    """

    kwargs = _get_kwargs(
        actor_key_id=actor_key_id,
        action=action,
        from_=from_,
        to=to,
        page=page,
        per_page=per_page,
        format_=format_,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    actor_key_id: Union[Unset, str] = UNSET,
    action: Union[Unset, str] = UNSET,
    from_: Union[Unset, str] = UNSET,
    to: Union[Unset, str] = UNSET,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
    format_: Union[Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    r"""GET /v1/audit — List audit log entries (admin scope required). Supports `?format=csv`.
    Live: `curl https://app.litegen.ai/api/v1/audit -H \"Authorization: Bearer sk_live_...\"`

    Args:
        actor_key_id (Union[Unset, str]):
        action (Union[Unset, str]):
        from_ (Union[Unset, str]):
        to (Union[Unset, str]):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):
        format_ (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseAuditLogEntry]
    """

    return sync_detailed(
        client=client,
        actor_key_id=actor_key_id,
        action=action,
        from_=from_,
        to=to,
        page=page,
        per_page=per_page,
        format_=format_,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    actor_key_id: Union[Unset, str] = UNSET,
    action: Union[Unset, str] = UNSET,
    from_: Union[Unset, str] = UNSET,
    to: Union[Unset, str] = UNSET,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
    format_: Union[Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    r"""GET /v1/audit — List audit log entries (admin scope required). Supports `?format=csv`.
    Live: `curl https://app.litegen.ai/api/v1/audit -H \"Authorization: Bearer sk_live_...\"`

    Args:
        actor_key_id (Union[Unset, str]):
        action (Union[Unset, str]):
        from_ (Union[Unset, str]):
        to (Union[Unset, str]):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):
        format_ (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]
    """

    kwargs = _get_kwargs(
        actor_key_id=actor_key_id,
        action=action,
        from_=from_,
        to=to,
        page=page,
        per_page=per_page,
        format_=format_,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    actor_key_id: Union[Unset, str] = UNSET,
    action: Union[Unset, str] = UNSET,
    from_: Union[Unset, str] = UNSET,
    to: Union[Unset, str] = UNSET,
    page: Union[Unset, int] = UNSET,
    per_page: Union[Unset, int] = UNSET,
    format_: Union[Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, PaginatedResponseAuditLogEntry]]:
    r"""GET /v1/audit — List audit log entries (admin scope required). Supports `?format=csv`.
    Live: `curl https://app.litegen.ai/api/v1/audit -H \"Authorization: Bearer sk_live_...\"`

    Args:
        actor_key_id (Union[Unset, str]):
        action (Union[Unset, str]):
        from_ (Union[Unset, str]):
        to (Union[Unset, str]):
        page (Union[Unset, int]):
        per_page (Union[Unset, int]):
        format_ (Union[Unset, str]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, PaginatedResponseAuditLogEntry]
    """

    return (
        await asyncio_detailed(
            client=client,
            actor_key_id=actor_key_id,
            action=action,
            from_=from_,
            to=to,
            page=page,
            per_page=per_page,
            format_=format_,
        )
    ).parsed
