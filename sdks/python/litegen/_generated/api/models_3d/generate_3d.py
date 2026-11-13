from http import HTTPStatus
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.model_3d_generation_request import Model3DGenerationRequest
from ...models.model_3d_generation_response import Model3DGenerationResponse
from ...types import UNSET, Response, Unset


def _get_kwargs(
    *,
    body: Model3DGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}
    if not isinstance(idempotency_key, Unset):
        headers["Idempotency-Key"] = idempotency_key

    if not isinstance(x_litegen_keep_outputs, Unset):
        headers["X-Litegen-Keep-Outputs"] = x_litegen_keep_outputs

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/models3d/generations",
    }

    _body = body.to_dict()

    _kwargs["json"] = _body
    headers["Content-Type"] = "application/json"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, Model3DGenerationResponse]]:
    if response.status_code == 200:
        response_200 = Model3DGenerationResponse.from_dict(response.json())

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 404:
        response_404 = ErrorResponse.from_dict(response.json())

        return response_404
    if response.status_code == 409:
        response_409 = ErrorResponse.from_dict(response.json())

        return response_409
    if response.status_code == 410:
        response_410 = ErrorResponse.from_dict(response.json())

        return response_410
    if response.status_code == 422:
        response_422 = ErrorResponse.from_dict(response.json())

        return response_422
    if response.status_code == 500:
        response_500 = ErrorResponse.from_dict(response.json())

        return response_500
    if response.status_code == 504:
        response_504 = ErrorResponse.from_dict(response.json())

        return response_504
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, Model3DGenerationResponse]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: Model3DGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, Model3DGenerationResponse]]:
    """POST /v1/models3d/generations — Start a 3D (mesh) generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (Model3DGenerationRequest): Request body for `POST /v1/models3d/generations` and
            `POST /v1/models3d/cost`.

            The mode is inferred, never passed: no `reference_images` is text-to-3D, an
            `init` image is image-to-3D, and `view-*` images are multiview-to-3D.

            Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
            `params` says which a model takes. One it does not take is refused with
            `param_unsupported` when `strict` is true (the default), and dropped — and
            named in the `X-Litegen-Dropped-Params` response header — when `strict` is
            false. A supported param with a bad VALUE is refused in both modes.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, Model3DGenerationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: Model3DGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, Model3DGenerationResponse]]:
    """POST /v1/models3d/generations — Start a 3D (mesh) generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (Model3DGenerationRequest): Request body for `POST /v1/models3d/generations` and
            `POST /v1/models3d/cost`.

            The mode is inferred, never passed: no `reference_images` is text-to-3D, an
            `init` image is image-to-3D, and `view-*` images are multiview-to-3D.

            Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
            `params` says which a model takes. One it does not take is refused with
            `param_unsupported` when `strict` is true (the default), and dropped — and
            named in the `X-Litegen-Dropped-Params` response header — when `strict` is
            false. A supported param with a bad VALUE is refused in both modes.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, Model3DGenerationResponse]
    """

    return sync_detailed(
        client=client,
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: Model3DGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, Model3DGenerationResponse]]:
    """POST /v1/models3d/generations — Start a 3D (mesh) generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (Model3DGenerationRequest): Request body for `POST /v1/models3d/generations` and
            `POST /v1/models3d/cost`.

            The mode is inferred, never passed: no `reference_images` is text-to-3D, an
            `init` image is image-to-3D, and `view-*` images are multiview-to-3D.

            Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
            `params` says which a model takes. One it does not take is refused with
            `param_unsupported` when `strict` is true (the default), and dropped — and
            named in the `X-Litegen-Dropped-Params` response header — when `strict` is
            false. A supported param with a bad VALUE is refused in both modes.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, Model3DGenerationResponse]]
    """

    kwargs = _get_kwargs(
        body=body,
        idempotency_key=idempotency_key,
        x_litegen_keep_outputs=x_litegen_keep_outputs,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: Model3DGenerationRequest,
    idempotency_key: Union[None, Unset, str] = UNSET,
    x_litegen_keep_outputs: Union[None, Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, Model3DGenerationResponse]]:
    """POST /v1/models3d/generations — Start a 3D (mesh) generation.

    Args:
        idempotency_key (Union[None, Unset, str]):
        x_litegen_keep_outputs (Union[None, Unset, str]):
        body (Model3DGenerationRequest): Request body for `POST /v1/models3d/generations` and
            `POST /v1/models3d/cost`.

            The mode is inferred, never passed: no `reference_images` is text-to-3D, an
            `init` image is image-to-3D, and `view-*` images are multiview-to-3D.

            Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
            `params` says which a model takes. One it does not take is refused with
            `param_unsupported` when `strict` is true (the default), and dropped — and
            named in the `X-Litegen-Dropped-Params` response header — when `strict` is
            false. A supported param with a bad VALUE is refused in both modes.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, Model3DGenerationResponse]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
            idempotency_key=idempotency_key,
            x_litegen_keep_outputs=x_litegen_keep_outputs,
        )
    ).parsed
