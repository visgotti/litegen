from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="GenerationAsset")


@_attrs_define
class GenerationAsset:
    """One stored file of a generation of any kind, as
    `GET /v1/generations/{id}/assets` lists it. litegen keeps every result for
    at least 7 days; `GET /v1/generations/{id}/assets/{index}` streams it with
    a litegen key alone, whatever the vendor's own URL did.

        Attributes:
            format_ (str): Lowercase container or image format, without a dot.
            index (int): Position in the list; the `{index}` that streams this file.
            kind (str): `image`, `video`, `mesh`, `texture`, `preview` or `original`. A plain
                string so a client tolerates a kind added later.
            url (str): Where the file is served from: absolute, and litegen's own storage.
            duration_s (Union[None, Unset, float]): Videos: length in seconds, when known.
            fps (Union[None, Unset, float]): Videos: frames per second, when known.
            height (Union[None, Unset, int]):
            sha256 (Union[None, Unset, str]): Lowercase hex SHA-256 of the stored file.
            size_bytes (Union[None, Unset, int]):
            width (Union[None, Unset, int]):
    """

    format_: str
    index: int
    kind: str
    url: str
    duration_s: Union[None, Unset, float] = UNSET
    fps: Union[None, Unset, float] = UNSET
    height: Union[None, Unset, int] = UNSET
    sha256: Union[None, Unset, str] = UNSET
    size_bytes: Union[None, Unset, int] = UNSET
    width: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        format_ = self.format_

        index = self.index

        kind = self.kind

        url = self.url

        duration_s: Union[None, Unset, float]
        if isinstance(self.duration_s, Unset):
            duration_s = UNSET
        else:
            duration_s = self.duration_s

        fps: Union[None, Unset, float]
        if isinstance(self.fps, Unset):
            fps = UNSET
        else:
            fps = self.fps

        height: Union[None, Unset, int]
        if isinstance(self.height, Unset):
            height = UNSET
        else:
            height = self.height

        sha256: Union[None, Unset, str]
        if isinstance(self.sha256, Unset):
            sha256 = UNSET
        else:
            sha256 = self.sha256

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
                "index": index,
                "kind": kind,
                "url": url,
            }
        )
        if duration_s is not UNSET:
            field_dict["duration_s"] = duration_s
        if fps is not UNSET:
            field_dict["fps"] = fps
        if height is not UNSET:
            field_dict["height"] = height
        if sha256 is not UNSET:
            field_dict["sha256"] = sha256
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes
        if width is not UNSET:
            field_dict["width"] = width

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        format_ = d.pop("format")

        index = d.pop("index")

        kind = d.pop("kind")

        url = d.pop("url")

        def _parse_duration_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        duration_s = _parse_duration_s(d.pop("duration_s", UNSET))

        def _parse_fps(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        fps = _parse_fps(d.pop("fps", UNSET))

        def _parse_height(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        height = _parse_height(d.pop("height", UNSET))

        def _parse_sha256(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        sha256 = _parse_sha256(d.pop("sha256", UNSET))

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

        generation_asset = cls(
            format_=format_,
            index=index,
            kind=kind,
            url=url,
            duration_s=duration_s,
            fps=fps,
            height=height,
            sha256=sha256,
            size_bytes=size_bytes,
            width=width,
        )

        generation_asset.additional_properties = d
        return generation_asset

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
