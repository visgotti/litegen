from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LayoutBox")


@_attrs_define
class LayoutBox:
    """Where a layout box sits, as fractions of the canvas (0 to 1, top-left
    origin): the output's size may not be known yet.

        Attributes:
            h (float):
            w (float):
            x (float):
            y (float):
    """

    h: float
    w: float
    x: float
    y: float
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        h = self.h

        w = self.w

        x = self.x

        y = self.y

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "h": h,
                "w": w,
                "x": x,
                "y": y,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        h = d.pop("h")

        w = d.pop("w")

        x = d.pop("x")

        y = d.pop("y")

        layout_box = cls(
            h=h,
            w=w,
            x=x,
            y=y,
        )

        layout_box.additional_properties = d
        return layout_box

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
