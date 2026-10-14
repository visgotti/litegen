from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.spend_report_response import SpendReportResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    group_by: Union[None, Unset, str] = UNSET,
    from_: Union[None, Unset, str] = UNSET,
    to: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
    app_id: Union[None, Unset, str] = UNSET,
    key_id: Union[None, Unset, str] = UNSET,
    user_id: Union[None, Unset, str] = UNSET,
    end_user: Union[None, Unset, str] = UNSET,
    model: Union[None, Unset, str] = UNSET,
    provider: Union[None, Unset, str] = UNSET,
    tag: Union[None, Unset, str] = UNSET,
    page: Union[None, Unset, int] = UNSET,
    per_page: Union[None, Unset, int] = UNSET,
) -> Dict[str, Any]:
    params: Dict[str, Any] = {}

    json_group_by: Union[None, Unset, str]
    if isinstance(group_by, Unset):
        json_group_by = UNSET
    else:
        json_group_by = group_by
    params["group_by"] = json_group_by

    json_from_: Union[None, Unset, str]
    if isinstance(from_, Unset):
        json_from_ = UNSET
    else:
        json_from_ = from_
    params["from"] = json_from_

    json_to: Union[None, Unset, str]
    if isinstance(to, Unset):
        json_to = UNSET
    else:
        json_to = to
    params["to"] = json_to

    json_org_id: Union[None, Unset, str]
    if isinstance(org_id, Unset):
        json_org_id = UNSET
    else:
        json_org_id = org_id
    params["org_id"] = json_org_id

    json_app_id: Union[None, Unset, str]
    if isinstance(app_id, Unset):
        json_app_id = UNSET
    else:
        json_app_id = app_id
    params["app_id"] = json_app_id

    json_key_id: Union[None, Unset, str]
    if isinstance(key_id, Unset):
        json_key_id = UNSET
    else:
        json_key_id = key_id
    params["key_id"] = json_key_id

    json_user_id: Union[None, Unset, str]
    if isinstance(user_id, Unset):
        json_user_id = UNSET
    else:
        json_user_id = user_id
    params["user_id"] = json_user_id

    json_end_user: Union[None, Unset, str]
    if isinstance(end_user, Unset):
        json_end_user = UNSET
    else:
        json_end_user = end_user
    params["end_user"] = json_end_user

    json_model: Union[None, Unset, str]
    if isinstance(model, Unset):
        json_model = UNSET
    else:
        json_model = model
    params["model"] = json_model

    json_provider: Union[None, Unset, str]
    if isinstance(provider, Unset):
        json_provider = UNSET
    else:
        json_provider = provider
    params["provider"] = json_provider

    json_tag: Union[None, Unset, str]
    if isinstance(tag, Unset):
        json_tag = UNSET
    else:
        json_tag = tag
    params["tag"] = json_tag

    json_page: Union[None, Unset, int]
    if isinstance(page, Unset):
        json_page = UNSET
    else:
        json_page = page
    params["page"] = json_page

    json_per_page: Union[None, Unset, int]
    if isinstance(per_page, Unset):
        json_per_page = UNSET
    else:
        json_per_page = per_page
    params["per_page"] = json_per_page

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "get",
        "url": "/v1/spend/report",
        "params": params,
    }

    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, SpendReportResponse]]:
    if response.status_code == 200:
        response_200 = SpendReportResponse.from_dict(response.json())

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
) -> Response[Union[ErrorResponse, SpendReportResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    group_by: Union[None, Unset, str] = UNSET,
    from_: Union[None, Unset, str] = UNSET,
    to: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
    app_id: Union[None, Unset, str] = UNSET,
    key_id: Union[None, Unset, str] = UNSET,
    user_id: Union[None, Unset, str] = UNSET,
    end_user: Union[None, Unset, str] = UNSET,
    model: Union[None, Unset, str] = UNSET,
    provider: Union[None, Unset, str] = UNSET,
    tag: Union[None, Unset, str] = UNSET,
    page: Union[None, Unset, int] = UNSET,
    per_page: Union[None, Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, SpendReportResponse]]:
    """GET /v1/spend/report — Spend and request counts grouped by one dimension.

    Args:
        group_by (Union[None, Unset, str]):
        from_ (Union[None, Unset, str]):
        to (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        app_id (Union[None, Unset, str]):
        key_id (Union[None, Unset, str]):
        user_id (Union[None, Unset, str]):
        end_user (Union[None, Unset, str]):
        model (Union[None, Unset, str]):
        provider (Union[None, Unset, str]):
        tag (Union[None, Unset, str]):
        page (Union[None, Unset, int]):
        per_page (Union[None, Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, SpendReportResponse]]
    """

    kwargs = _get_kwargs(
        group_by=group_by,
        from_=from_,
        to=to,
        org_id=org_id,
        app_id=app_id,
        key_id=key_id,
        user_id=user_id,
        end_user=end_user,
        model=model,
        provider=provider,
        tag=tag,
        page=page,
        per_page=per_page,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    group_by: Union[None, Unset, str] = UNSET,
    from_: Union[None, Unset, str] = UNSET,
    to: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
    app_id: Union[None, Unset, str] = UNSET,
    key_id: Union[None, Unset, str] = UNSET,
    user_id: Union[None, Unset, str] = UNSET,
    end_user: Union[None, Unset, str] = UNSET,
    model: Union[None, Unset, str] = UNSET,
    provider: Union[None, Unset, str] = UNSET,
    tag: Union[None, Unset, str] = UNSET,
    page: Union[None, Unset, int] = UNSET,
    per_page: Union[None, Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, SpendReportResponse]]:
    """GET /v1/spend/report — Spend and request counts grouped by one dimension.

    Args:
        group_by (Union[None, Unset, str]):
        from_ (Union[None, Unset, str]):
        to (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        app_id (Union[None, Unset, str]):
        key_id (Union[None, Unset, str]):
        user_id (Union[None, Unset, str]):
        end_user (Union[None, Unset, str]):
        model (Union[None, Unset, str]):
        provider (Union[None, Unset, str]):
        tag (Union[None, Unset, str]):
        page (Union[None, Unset, int]):
        per_page (Union[None, Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, SpendReportResponse]
    """

    return sync_detailed(
        client=client,
        group_by=group_by,
        from_=from_,
        to=to,
        org_id=org_id,
        app_id=app_id,
        key_id=key_id,
        user_id=user_id,
        end_user=end_user,
        model=model,
        provider=provider,
        tag=tag,
        page=page,
        per_page=per_page,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    group_by: Union[None, Unset, str] = UNSET,
    from_: Union[None, Unset, str] = UNSET,
    to: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
    app_id: Union[None, Unset, str] = UNSET,
    key_id: Union[None, Unset, str] = UNSET,
    user_id: Union[None, Unset, str] = UNSET,
    end_user: Union[None, Unset, str] = UNSET,
    model: Union[None, Unset, str] = UNSET,
    provider: Union[None, Unset, str] = UNSET,
    tag: Union[None, Unset, str] = UNSET,
    page: Union[None, Unset, int] = UNSET,
    per_page: Union[None, Unset, int] = UNSET,
) -> Response[Union[ErrorResponse, SpendReportResponse]]:
    """GET /v1/spend/report — Spend and request counts grouped by one dimension.

    Args:
        group_by (Union[None, Unset, str]):
        from_ (Union[None, Unset, str]):
        to (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        app_id (Union[None, Unset, str]):
        key_id (Union[None, Unset, str]):
        user_id (Union[None, Unset, str]):
        end_user (Union[None, Unset, str]):
        model (Union[None, Unset, str]):
        provider (Union[None, Unset, str]):
        tag (Union[None, Unset, str]):
        page (Union[None, Unset, int]):
        per_page (Union[None, Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, SpendReportResponse]]
    """

    kwargs = _get_kwargs(
        group_by=group_by,
        from_=from_,
        to=to,
        org_id=org_id,
        app_id=app_id,
        key_id=key_id,
        user_id=user_id,
        end_user=end_user,
        model=model,
        provider=provider,
        tag=tag,
        page=page,
        per_page=per_page,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    group_by: Union[None, Unset, str] = UNSET,
    from_: Union[None, Unset, str] = UNSET,
    to: Union[None, Unset, str] = UNSET,
    org_id: Union[None, Unset, str] = UNSET,
    app_id: Union[None, Unset, str] = UNSET,
    key_id: Union[None, Unset, str] = UNSET,
    user_id: Union[None, Unset, str] = UNSET,
    end_user: Union[None, Unset, str] = UNSET,
    model: Union[None, Unset, str] = UNSET,
    provider: Union[None, Unset, str] = UNSET,
    tag: Union[None, Unset, str] = UNSET,
    page: Union[None, Unset, int] = UNSET,
    per_page: Union[None, Unset, int] = UNSET,
) -> Optional[Union[ErrorResponse, SpendReportResponse]]:
    """GET /v1/spend/report — Spend and request counts grouped by one dimension.

    Args:
        group_by (Union[None, Unset, str]):
        from_ (Union[None, Unset, str]):
        to (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        app_id (Union[None, Unset, str]):
        key_id (Union[None, Unset, str]):
        user_id (Union[None, Unset, str]):
        end_user (Union[None, Unset, str]):
        model (Union[None, Unset, str]):
        provider (Union[None, Unset, str]):
        tag (Union[None, Unset, str]):
        page (Union[None, Unset, int]):
        per_page (Union[None, Unset, int]):

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, SpendReportResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            group_by=group_by,
            from_=from_,
            to=to,
            org_id=org_id,
            app_id=app_id,
            key_id=key_id,
            user_id=user_id,
            end_user=end_user,
            model=model,
            provider=provider,
            tag=tag,
            page=page,
            per_page=per_page,
        )
    ).parsed
