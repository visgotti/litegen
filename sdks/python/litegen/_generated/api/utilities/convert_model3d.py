from http import HTTPStatus
from io import BytesIO
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.error_response import ErrorResponse
from ...models.model_3d_format import Model3DFormat
from ...types import UNSET, File, Response


def _get_kwargs(
    *,
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    params: Dict[str, Any] = {}

    json_from_ = from_.value
    params["from"] = json_from_

    json_to = to.value
    params["to"] = json_to

    params = {k: v for k, v in params.items() if v is not UNSET and v is not None}

    _kwargs: Dict[str, Any] = {
        "method": "post",
        "url": "/v1/utils/models3d/convert",
        "params": params,
    }

    _body = body.payload

    _kwargs["content"] = _body
    headers["Content-Type"] = "application/octet-stream"

    _kwargs["headers"] = headers
    return _kwargs


def _parse_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Optional[Union[ErrorResponse, File]]:
    if response.status_code == 200:
        response_200 = File(payload=BytesIO(response.content))

        return response_200
    if response.status_code == 400:
        response_400 = ErrorResponse.from_dict(response.json())

        return response_400
    if response.status_code == 413:
        response_413 = ErrorResponse.from_dict(response.json())

        return response_413
    if response.status_code == 422:
        response_422 = ErrorResponse.from_dict(response.json())

        return response_422
    if response.status_code == 503:
        response_503 = ErrorResponse.from_dict(response.json())

        return response_503
    if client.raise_on_unexpected_status:
        raise errors.UnexpectedStatus(response.status_code, response.content)
    else:
        return None


def _build_response(
    *, client: Union[AuthenticatedClient, Client], response: httpx.Response
) -> Response[Union[ErrorResponse, File]]:
    return Response(
        status_code=HTTPStatus(response.status_code),
        content=response.content,
        headers=response.headers,
        parsed=_parse_response(client=client, response=response),
    )


def sync_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
) -> Response[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`.

    Live: `curl --data-binary @fox.glb
    \"https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj\" -H \"Authorization: Bearer
    sk_live_...\" -o fox.obj`

    Args:
        from_ (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        to (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        body (File): A mesh file as raw bytes, for the spec only: the handler reads the `Body`
            directly. Without this, `Vec<u8>` documents as an array of integers and the
            SDK generators type the upload as `number[]`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, File]]
    """

    kwargs = _get_kwargs(
        body=body,
        from_=from_,
        to=to,
    )

    response = client.get_httpx_client().request(
        **kwargs,
    )

    return _build_response(client=client, response=response)


def sync(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
) -> Optional[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`.

    Live: `curl --data-binary @fox.glb
    \"https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj\" -H \"Authorization: Bearer
    sk_live_...\" -o fox.obj`

    Args:
        from_ (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        to (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        body (File): A mesh file as raw bytes, for the spec only: the handler reads the `Body`
            directly. Without this, `Vec<u8>` documents as an array of integers and the
            SDK generators type the upload as `number[]`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, File]
    """

    return sync_detailed(
        client=client,
        body=body,
        from_=from_,
        to=to,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
) -> Response[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`.

    Live: `curl --data-binary @fox.glb
    \"https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj\" -H \"Authorization: Bearer
    sk_live_...\" -o fox.obj`

    Args:
        from_ (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        to (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        body (File): A mesh file as raw bytes, for the spec only: the handler reads the `Body`
            directly. Without this, `Vec<u8>` documents as an array of integers and the
            SDK generators type the upload as `number[]`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Response[Union[ErrorResponse, File]]
    """

    kwargs = _get_kwargs(
        body=body,
        from_=from_,
        to=to,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
) -> Optional[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`.

    Live: `curl --data-binary @fox.glb
    \"https://app.litegen.ai/api/v1/utils/models3d/convert?from=glb&to=obj\" -H \"Authorization: Bearer
    sk_live_...\" -o fox.obj`

    Args:
        from_ (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        to (Model3DFormat): Every mesh container a 3D request may name in `output_formats`.

            The closed vocabulary across all of litegen's 3D models. Which subset one
            model can deliver is per model: `GET /v1/models/{id}` lists it under
            `params.output_formats.enum_values`, and anything outside it is refused
            with `param_enum_mismatch` before any vendor is called.

            A container reaches a caller one of two ways. Either litegen derives it
            locally from whatever readable container the vendor emitted — the set
            `proxy::model3d_convert::derivable_formats` returns, served per model as
            `params.output_formats.derivable_values` — and any number of those cost one
            vendor job together; or the vendor emits it itself, one job each. Which is
            which grows with `litegen-core/src/mesh/`, so this list is deliberately a
            superset of every container any vendor or the converter produces.

            Requests carry these as plain, case-insensitive strings — validation, not
            deserialisation, owns the error, so it can name what IS deliverable. This
            type pins the vocabulary for OpenAPI and for the catalog loader.
        body (File): A mesh file as raw bytes, for the spec only: the handler reads the `Body`
            directly. Without this, `Vec<u8>` documents as an array of integers and the
            SDK generators type the upload as `number[]`.

    Raises:
        errors.UnexpectedStatus: If the server returns an undocumented status code and Client.raise_on_unexpected_status is True.
        httpx.TimeoutException: If the request takes longer than Client.timeout.

    Returns:
        Union[ErrorResponse, File]
    """

    return (
        await asyncio_detailed(
            client=client,
            body=body,
            from_=from_,
            to=to,
        )
    ).parsed
