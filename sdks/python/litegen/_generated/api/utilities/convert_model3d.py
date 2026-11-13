from http import HTTPStatus
from io import BytesIO
from typing import Any, Dict, Optional, Union

import httpx

from ... import errors
from ...client import AuthenticatedClient, Client
from ...models.convert_model_3d_ktx_2_textures import ConvertModel3DKtx2Textures
from ...models.convert_model_3d_package import ConvertModel3DPackage
from ...models.error_response import ErrorResponse
from ...models.model_3d_format import Model3DFormat
from ...types import UNSET, File, Response, Unset


def _get_kwargs(
    *,
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
    units: Union[Unset, str] = UNSET,
    up: Union[Unset, str] = UNSET,
    from_units: Union[Unset, str] = UNSET,
    from_up: Union[Unset, str] = UNSET,
    fbx_color_space: Union[Unset, str] = UNSET,
    package: Union[Unset, ConvertModel3DPackage] = UNSET,
    ktx2_textures: Union[Unset, ConvertModel3DKtx2Textures] = UNSET,
    fbx_light_units: Union[Unset, str] = UNSET,
    usd_light_units: Union[Unset, str] = UNSET,
    dae_morphs: Union[Unset, str] = UNSET,
    usdz_root: Union[Unset, str] = UNSET,
    usd_clips: Union[Unset, str] = UNSET,
) -> Dict[str, Any]:
    headers: Dict[str, Any] = {}

    params: Dict[str, Any] = {}

    json_from_ = from_.value
    params["from"] = json_from_

    json_to = to.value
    params["to"] = json_to

    params["units"] = units

    params["up"] = up

    params["from_units"] = from_units

    params["from_up"] = from_up

    params["fbx_color_space"] = fbx_color_space

    json_package: Union[Unset, str] = UNSET
    if not isinstance(package, Unset):
        json_package = package.value

    params["package"] = json_package

    json_ktx2_textures: Union[Unset, str] = UNSET
    if not isinstance(ktx2_textures, Unset):
        json_ktx2_textures = ktx2_textures.value

    params["ktx2_textures"] = json_ktx2_textures

    params["fbx_light_units"] = fbx_light_units

    params["usd_light_units"] = usd_light_units

    params["dae_morphs"] = dae_morphs

    params["usdz_root"] = usdz_root

    params["usd_clips"] = usd_clips

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
    if response.status_code == 408:
        response_408 = ErrorResponse.from_dict(response.json())

        return response_408
    if response.status_code == 413:
        response_413 = ErrorResponse.from_dict(response.json())

        return response_413
    if response.status_code == 422:
        response_422 = ErrorResponse.from_dict(response.json())

        return response_422
    if response.status_code == 500:
        response_500 = ErrorResponse.from_dict(response.json())

        return response_500
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
    units: Union[Unset, str] = UNSET,
    up: Union[Unset, str] = UNSET,
    from_units: Union[Unset, str] = UNSET,
    from_up: Union[Unset, str] = UNSET,
    fbx_color_space: Union[Unset, str] = UNSET,
    package: Union[Unset, ConvertModel3DPackage] = UNSET,
    ktx2_textures: Union[Unset, ConvertModel3DKtx2Textures] = UNSET,
    fbx_light_units: Union[Unset, str] = UNSET,
    usd_light_units: Union[Unset, str] = UNSET,
    dae_morphs: Union[Unset, str] = UNSET,
    usdz_root: Union[Unset, str] = UNSET,
    usd_clips: Union[Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`. WebP and KTX2 (Basis Universal)
    textures are re-encoded as PNG, as generations deliver them, so no
    container drops them and, by default, no glTF requires
    `EXT_texture_webp` or `KHR_texture_basisu` (`ktx2_textures=keep` leaves a
    KTX2 in a glb or gltf, which then requires `KHR_texture_basisu`). By
    default the output is one file: OBJ carries its materials as vertex
    colours, and Collada, USDA and USDC leave textures out (named as losses).
    `package=zip` answers with a zip of the file and its sidecars instead,
    which keeps them. A USD file (`.usda`, `.usdc`, or a `.usd` holding either)
    is read with `from=usda` or `from=usdc`, whichever encoding it is in.

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
        units (Union[Unset, str]):
        up (Union[Unset, str]):
        from_units (Union[Unset, str]):
        from_up (Union[Unset, str]):
        fbx_color_space (Union[Unset, str]):
        package (Union[Unset, ConvertModel3DPackage]): What the converter answers with
            (`package`).
        ktx2_textures (Union[Unset, ConvertModel3DKtx2Textures]): What the converter does with a
            KTX2 (Basis Universal) texture
            (`ktx2_textures`).
        fbx_light_units (Union[Unset, str]):
        usd_light_units (Union[Unset, str]):
        dae_morphs (Union[Unset, str]):
        usdz_root (Union[Unset, str]):
        usd_clips (Union[Unset, str]):
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
        units=units,
        up=up,
        from_units=from_units,
        from_up=from_up,
        fbx_color_space=fbx_color_space,
        package=package,
        ktx2_textures=ktx2_textures,
        fbx_light_units=fbx_light_units,
        usd_light_units=usd_light_units,
        dae_morphs=dae_morphs,
        usdz_root=usdz_root,
        usd_clips=usd_clips,
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
    units: Union[Unset, str] = UNSET,
    up: Union[Unset, str] = UNSET,
    from_units: Union[Unset, str] = UNSET,
    from_up: Union[Unset, str] = UNSET,
    fbx_color_space: Union[Unset, str] = UNSET,
    package: Union[Unset, ConvertModel3DPackage] = UNSET,
    ktx2_textures: Union[Unset, ConvertModel3DKtx2Textures] = UNSET,
    fbx_light_units: Union[Unset, str] = UNSET,
    usd_light_units: Union[Unset, str] = UNSET,
    dae_morphs: Union[Unset, str] = UNSET,
    usdz_root: Union[Unset, str] = UNSET,
    usd_clips: Union[Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`. WebP and KTX2 (Basis Universal)
    textures are re-encoded as PNG, as generations deliver them, so no
    container drops them and, by default, no glTF requires
    `EXT_texture_webp` or `KHR_texture_basisu` (`ktx2_textures=keep` leaves a
    KTX2 in a glb or gltf, which then requires `KHR_texture_basisu`). By
    default the output is one file: OBJ carries its materials as vertex
    colours, and Collada, USDA and USDC leave textures out (named as losses).
    `package=zip` answers with a zip of the file and its sidecars instead,
    which keeps them. A USD file (`.usda`, `.usdc`, or a `.usd` holding either)
    is read with `from=usda` or `from=usdc`, whichever encoding it is in.

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
        units (Union[Unset, str]):
        up (Union[Unset, str]):
        from_units (Union[Unset, str]):
        from_up (Union[Unset, str]):
        fbx_color_space (Union[Unset, str]):
        package (Union[Unset, ConvertModel3DPackage]): What the converter answers with
            (`package`).
        ktx2_textures (Union[Unset, ConvertModel3DKtx2Textures]): What the converter does with a
            KTX2 (Basis Universal) texture
            (`ktx2_textures`).
        fbx_light_units (Union[Unset, str]):
        usd_light_units (Union[Unset, str]):
        dae_morphs (Union[Unset, str]):
        usdz_root (Union[Unset, str]):
        usd_clips (Union[Unset, str]):
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
        units=units,
        up=up,
        from_units=from_units,
        from_up=from_up,
        fbx_color_space=fbx_color_space,
        package=package,
        ktx2_textures=ktx2_textures,
        fbx_light_units=fbx_light_units,
        usd_light_units=usd_light_units,
        dae_morphs=dae_morphs,
        usdz_root=usdz_root,
        usd_clips=usd_clips,
    ).parsed


async def asyncio_detailed(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
    units: Union[Unset, str] = UNSET,
    up: Union[Unset, str] = UNSET,
    from_units: Union[Unset, str] = UNSET,
    from_up: Union[Unset, str] = UNSET,
    fbx_color_space: Union[Unset, str] = UNSET,
    package: Union[Unset, ConvertModel3DPackage] = UNSET,
    ktx2_textures: Union[Unset, ConvertModel3DKtx2Textures] = UNSET,
    fbx_light_units: Union[Unset, str] = UNSET,
    usd_light_units: Union[Unset, str] = UNSET,
    dae_morphs: Union[Unset, str] = UNSET,
    usdz_root: Union[Unset, str] = UNSET,
    usd_clips: Union[Unset, str] = UNSET,
) -> Response[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`. WebP and KTX2 (Basis Universal)
    textures are re-encoded as PNG, as generations deliver them, so no
    container drops them and, by default, no glTF requires
    `EXT_texture_webp` or `KHR_texture_basisu` (`ktx2_textures=keep` leaves a
    KTX2 in a glb or gltf, which then requires `KHR_texture_basisu`). By
    default the output is one file: OBJ carries its materials as vertex
    colours, and Collada, USDA and USDC leave textures out (named as losses).
    `package=zip` answers with a zip of the file and its sidecars instead,
    which keeps them. A USD file (`.usda`, `.usdc`, or a `.usd` holding either)
    is read with `from=usda` or `from=usdc`, whichever encoding it is in.

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
        units (Union[Unset, str]):
        up (Union[Unset, str]):
        from_units (Union[Unset, str]):
        from_up (Union[Unset, str]):
        fbx_color_space (Union[Unset, str]):
        package (Union[Unset, ConvertModel3DPackage]): What the converter answers with
            (`package`).
        ktx2_textures (Union[Unset, ConvertModel3DKtx2Textures]): What the converter does with a
            KTX2 (Basis Universal) texture
            (`ktx2_textures`).
        fbx_light_units (Union[Unset, str]):
        usd_light_units (Union[Unset, str]):
        dae_morphs (Union[Unset, str]):
        usdz_root (Union[Unset, str]):
        usd_clips (Union[Unset, str]):
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
        units=units,
        up=up,
        from_units=from_units,
        from_up=from_up,
        fbx_color_space=fbx_color_space,
        package=package,
        ktx2_textures=ktx2_textures,
        fbx_light_units=fbx_light_units,
        usd_light_units=usd_light_units,
        dae_morphs=dae_morphs,
        usdz_root=usdz_root,
        usd_clips=usd_clips,
    )

    response = await client.get_async_httpx_client().request(**kwargs)

    return _build_response(client=client, response=response)


async def asyncio(
    *,
    client: Union[AuthenticatedClient, Client],
    body: File,
    from_: Model3DFormat,
    to: Model3DFormat,
    units: Union[Unset, str] = UNSET,
    up: Union[Unset, str] = UNSET,
    from_units: Union[Unset, str] = UNSET,
    from_up: Union[Unset, str] = UNSET,
    fbx_color_space: Union[Unset, str] = UNSET,
    package: Union[Unset, ConvertModel3DPackage] = UNSET,
    ktx2_textures: Union[Unset, ConvertModel3DKtx2Textures] = UNSET,
    fbx_light_units: Union[Unset, str] = UNSET,
    usd_light_units: Union[Unset, str] = UNSET,
    dae_morphs: Union[Unset, str] = UNSET,
    usdz_root: Union[Unset, str] = UNSET,
    usd_clips: Union[Unset, str] = UNSET,
) -> Optional[Union[ErrorResponse, File]]:
    r"""POST /v1/utils/models3d/convert — Convert a mesh between containers without generating anything.

     The body is the raw file. The response is the converted file, with the
    target's `Content-Type`. Whatever the target container can hold survives
    (materials, textures, skins, animation); what it cannot is dropped and
    named in `X-Litegen-Conversion-Losses`. WebP and KTX2 (Basis Universal)
    textures are re-encoded as PNG, as generations deliver them, so no
    container drops them and, by default, no glTF requires
    `EXT_texture_webp` or `KHR_texture_basisu` (`ktx2_textures=keep` leaves a
    KTX2 in a glb or gltf, which then requires `KHR_texture_basisu`). By
    default the output is one file: OBJ carries its materials as vertex
    colours, and Collada, USDA and USDC leave textures out (named as losses).
    `package=zip` answers with a zip of the file and its sidecars instead,
    which keeps them. A USD file (`.usda`, `.usdc`, or a `.usd` holding either)
    is read with `from=usda` or `from=usdc`, whichever encoding it is in.

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
        units (Union[Unset, str]):
        up (Union[Unset, str]):
        from_units (Union[Unset, str]):
        from_up (Union[Unset, str]):
        fbx_color_space (Union[Unset, str]):
        package (Union[Unset, ConvertModel3DPackage]): What the converter answers with
            (`package`).
        ktx2_textures (Union[Unset, ConvertModel3DKtx2Textures]): What the converter does with a
            KTX2 (Basis Universal) texture
            (`ktx2_textures`).
        fbx_light_units (Union[Unset, str]):
        usd_light_units (Union[Unset, str]):
        dae_morphs (Union[Unset, str]):
        usdz_root (Union[Unset, str]):
        usd_clips (Union[Unset, str]):
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
            units=units,
            up=up,
            from_units=from_units,
            from_up=from_up,
            fbx_color_space=fbx_color_space,
            package=package,
            ktx2_textures=ktx2_textures,
            fbx_light_units=fbx_light_units,
            usd_light_units=usd_light_units,
            dae_morphs=dae_morphs,
            usdz_root=usdz_root,
            usd_clips=usd_clips,
        )
    ).parsed
