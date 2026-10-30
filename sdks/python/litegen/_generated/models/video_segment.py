from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.segment_origin import SegmentOrigin

T = TypeVar("T", bound="VideoSegment")


@_attrs_define
class VideoSegment:
    """One stretch of a finished operation's clip, in its own seconds.

    Attributes:
        end_s (float):
        origin (SegmentOrigin): Where a stretch of a finished clip came from (3D doc §5).
        start_s (float):
    """

    end_s: float
    origin: SegmentOrigin
    start_s: float
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        end_s = self.end_s

        origin = self.origin.value

        start_s = self.start_s

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "end_s": end_s,
                "origin": origin,
                "start_s": start_s,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        end_s = d.pop("end_s")

        origin = SegmentOrigin(d.pop("origin"))

        start_s = d.pop("start_s")

        video_segment = cls(
            end_s=end_s,
            origin=origin,
            start_s=start_s,
        )

        video_segment.additional_properties = d
        return video_segment

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
