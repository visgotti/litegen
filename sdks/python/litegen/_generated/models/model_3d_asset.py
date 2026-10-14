from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.model_3d_asset_kind import Model3DAssetKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="Model3DAsset")


@_attrs_define
class Model3DAsset:
    """One file produced by a 3D generation, re-hosted on litegen storage (the
    app's own bucket when it configured one).

    A `completed` generation carries one `mesh` asset for every format in its
    `output_formats` — never fewer; a missing one fails the generation instead —
    plus whatever textures and previews the vendor returned.

        Attributes:
            format_ (str): Lowercase container without a dot. Meshes use the `Model3dFormat`
                vocabulary; textures and previews their image format (`png`, `jpg`,
                `webp`). A plain string so a client tolerates a format added later.
            kind (Model3DAssetKind): What a `Model3dAsset` is.
            url (str): Always absolute: a root-relative path is indistinguishable from an
                outage to a client fetching it from a worker. Meshes live at
                `…/{generation_id}/model.{format}`.
            height (Union[None, Unset, int]): Pixel height, on textures and previews whose vendor reports it.
            polycount (Union[None, Unset, int]): Triangle count, on meshes whose vendor reports one. A container litegen
                derived carries its source's count — conversion keeps every triangle.
            size_bytes (Union[None, Unset, int]): Size of the stored file.
            width (Union[None, Unset, int]): Pixel width, on textures and previews whose vendor reports it.
    """

    format_: str
    kind: Model3DAssetKind
    url: str
    height: Union[None, Unset, int] = UNSET
    polycount: Union[None, Unset, int] = UNSET
    size_bytes: Union[None, Unset, int] = UNSET
    width: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        format_ = self.format_

        kind = self.kind.value

        url = self.url

        height: Union[None, Unset, int]
        if isinstance(self.height, Unset):
            height = UNSET
        else:
            height = self.height

        polycount: Union[None, Unset, int]
        if isinstance(self.polycount, Unset):
            polycount = UNSET
        else:
            polycount = self.polycount

        size_bytes: Union[None, Unset, int]
        if isinstance(self.size_bytes, Unset):
            size_bytes = UNSET
        else:
            size_bytes = self.size_bytes

        width: Union[None, Unset, int]
        if isinstance(self.width, Unset):
            width = UNSET
        else:
            width = self.width

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "format": format_,
                "kind": kind,
                "url": url,
            }
        )
        if height is not UNSET:
            field_dict["height"] = height
        if polycount is not UNSET:
            field_dict["polycount"] = polycount
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes
        if width is not UNSET:
            field_dict["width"] = width

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        format_ = d.pop("format")

        kind = Model3DAssetKind(d.pop("kind"))

        url = d.pop("url")

        def _parse_height(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        height = _parse_height(d.pop("height", UNSET))

        def _parse_polycount(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        polycount = _parse_polycount(d.pop("polycount", UNSET))

        def _parse_size_bytes(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        def _parse_width(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        width = _parse_width(d.pop("width", UNSET))

        model_3d_asset = cls(
            format_=format_,
            kind=kind,
            url=url,
            height=height,
            polycount=polycount,
            size_bytes=size_bytes,
            width=width,
        )

        model_3d_asset.additional_properties = d
        return model_3d_asset

    @property
    def additional_keys(self) -> List[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
