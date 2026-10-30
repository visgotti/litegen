from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.segmentation_prompt_type_0_type import SegmentationPromptType0Type
from ..types import UNSET, Unset

T = TypeVar("T", bound="SegmentationPromptType0")


@_attrs_define
class SegmentationPromptType0:
    """A spot on the object (`label: 1`, the default) or one to leave out
    of it (`label: 0`).

        Attributes:
            type (SegmentationPromptType0Type):
            x (int):
            y (int):
            label (Union[Unset, int]):
    """

    type: SegmentationPromptType0Type
    x: int
    y: int
    label: Union[Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        type = self.type.value

        x = self.x

        y = self.y

        label = self.label

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "type": type,
                "x": x,
                "y": y,
            }
        )
        if label is not UNSET:
            field_dict["label"] = label

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        type = SegmentationPromptType0Type(d.pop("type"))

        x = d.pop("x")

        y = d.pop("y")

        label = d.pop("label", UNSET)

        segmentation_prompt_type_0 = cls(
            type=type,
            x=x,
            y=y,
            label=label,
        )

        segmentation_prompt_type_0.additional_properties = d
        return segmentation_prompt_type_0

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
