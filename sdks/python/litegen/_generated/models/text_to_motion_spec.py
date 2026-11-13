from typing import Any, Dict, Type, TypeVar

from attrs import define as _attrs_define

T = TypeVar("T", bound="TextToMotionSpec")


@_attrs_define
class TextToMotionSpec:
    """
    Attributes:
        max_duration_s (float):
        min_duration_s (float):
    """

    max_duration_s: float
    min_duration_s: float

    def to_dict(self) -> Dict[str, Any]:
        max_duration_s = self.max_duration_s

        min_duration_s = self.min_duration_s

        field_dict: Dict[str, Any] = {}
        field_dict.update(
            {
                "max_duration_s": max_duration_s,
                "min_duration_s": min_duration_s,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_duration_s = d.pop("max_duration_s")

        min_duration_s = d.pop("min_duration_s")

        text_to_motion_spec = cls(
            max_duration_s=max_duration_s,
            min_duration_s=min_duration_s,
        )

        return text_to_motion_spec
