from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Expand")


@_attrs_define
class Expand:
    """Pixels to add to each side of the init image's canvas (3D doc §4): the
    result is the whole canvas, with the source at (`left`, `top`).

        Attributes:
            bottom (Union[Unset, int]):
            left (Union[Unset, int]):
            right (Union[Unset, int]):
            top (Union[Unset, int]):
    """

    bottom: Union[Unset, int] = UNSET
    left: Union[Unset, int] = UNSET
    right: Union[Unset, int] = UNSET
    top: Union[Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        bottom = self.bottom

        left = self.left

        right = self.right

        top = self.top

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if bottom is not UNSET:
            field_dict["bottom"] = bottom
        if left is not UNSET:
            field_dict["left"] = left
        if right is not UNSET:
            field_dict["right"] = right
        if top is not UNSET:
            field_dict["top"] = top

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        bottom = d.pop("bottom", UNSET)

        left = d.pop("left", UNSET)

        right = d.pop("right", UNSET)

        top = d.pop("top", UNSET)

        expand = cls(
            bottom=bottom,
            left=left,
            right=right,
            top=top,
        )

        expand.additional_properties = d
        return expand

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
