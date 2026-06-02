from typing import Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="SizeSpecEnum")


@_attrs_define
class SizeSpecEnum:
    """
    Attributes:
        values (List[List[int]]):
    """

    values: List[List[int]]
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        values = []
        for values_item_data in self.values:
            values_item = values_item_data

            values.append(values_item)

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "values": values,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        values = []
        _values = d.pop("values")
        for values_item_data in _values:
            values_item = cast(List[int], values_item_data)

            values.append(values_item)

        size_spec_enum = cls(
            values=values,
        )

        size_spec_enum.additional_properties = d
        return size_spec_enum

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
