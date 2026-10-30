from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="VideoMedia")


@_attrs_define
class VideoMedia:
    """What a finished video is, read from its bytes (PixelGenie L10).

    Attributes:
        duration_s (Union[None, Unset, float]):
        fps (Union[None, Unset, float]):
        has_audio (Union[None, Unset, bool]):
        height (Union[None, Unset, int]):
        width (Union[None, Unset, int]):
    """

    duration_s: Union[None, Unset, float] = UNSET
    fps: Union[None, Unset, float] = UNSET
    has_audio: Union[None, Unset, bool] = UNSET
    height: Union[None, Unset, int] = UNSET
    width: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
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

        has_audio: Union[None, Unset, bool]
        if isinstance(self.has_audio, Unset):
            has_audio = UNSET
        else:
            has_audio = self.has_audio

        height: Union[None, Unset, int]
        if isinstance(self.height, Unset):
            height = UNSET
        else:
            height = self.height

        width: Union[None, Unset, int]
        if isinstance(self.width, Unset):
            width = UNSET
        else:
            width = self.width

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if duration_s is not UNSET:
            field_dict["duration_s"] = duration_s
        if fps is not UNSET:
            field_dict["fps"] = fps
        if has_audio is not UNSET:
            field_dict["has_audio"] = has_audio
        if height is not UNSET:
            field_dict["height"] = height
        if width is not UNSET:
            field_dict["width"] = width

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

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

        def _parse_has_audio(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        has_audio = _parse_has_audio(d.pop("has_audio", UNSET))

        def _parse_height(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        height = _parse_height(d.pop("height", UNSET))

        def _parse_width(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        width = _parse_width(d.pop("width", UNSET))

        video_media = cls(
            duration_s=duration_s,
            fps=fps,
            has_audio=has_audio,
            height=height,
            width=width,
        )

        video_media.additional_properties = d
        return video_media

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
