from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

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
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_duration_s = self.max_duration_s

        min_duration_s = self.min_duration_s

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
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

        text_to_motion_spec.additional_properties = d
        return text_to_motion_spec

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
