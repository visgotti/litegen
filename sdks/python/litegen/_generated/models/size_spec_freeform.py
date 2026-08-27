from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SizeSpecFreeform")


@_attrs_define
class SizeSpecFreeform:
    """
    Attributes:
        max_height (int):
        max_width (int):
        min_height (int):
        min_width (int):
        description (Union[None, Unset, str]):
        label (Union[None, Unset, str]):
        multiple_of (Union[None, Unset, int]):
    """

    max_height: int
    max_width: int
    min_height: int
    min_width: int
    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    multiple_of: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_height = self.max_height

        max_width = self.max_width

        min_height = self.min_height

        min_width = self.min_width

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        multiple_of: Union[None, Unset, int]
        if isinstance(self.multiple_of, Unset):
            multiple_of = UNSET
        else:
            multiple_of = self.multiple_of

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_height": max_height,
                "max_width": max_width,
                "min_height": min_height,
                "min_width": min_width,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if multiple_of is not UNSET:
            field_dict["multiple_of"] = multiple_of

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_height = d.pop("max_height")

        max_width = d.pop("max_width")

        min_height = d.pop("min_height")

        min_width = d.pop("min_width")

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_multiple_of(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        multiple_of = _parse_multiple_of(d.pop("multiple_of", UNSET))

        size_spec_freeform = cls(
            max_height=max_height,
            max_width=max_width,
            min_height=min_height,
            min_width=min_width,
            description=description,
            label=label,
            multiple_of=multiple_of,
        )

        size_spec_freeform.additional_properties = d
        return size_spec_freeform

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
