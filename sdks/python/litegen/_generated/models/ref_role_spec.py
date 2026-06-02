from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="RefRoleSpec")


@_attrs_define
class RefRoleSpec:
    """
    Attributes:
        max_count (int):
        min_count (int):
        required (Union[Unset, bool]):
    """

    max_count: int
    min_count: int
    required: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_count = self.max_count

        min_count = self.min_count

        required = self.required

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_count": max_count,
                "min_count": min_count,
            }
        )
        if required is not UNSET:
            field_dict["required"] = required

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_count = d.pop("max_count")

        min_count = d.pop("min_count")

        required = d.pop("required", UNSET)

        ref_role_spec = cls(
            max_count=max_count,
            min_count=min_count,
            required=required,
        )

        ref_role_spec.additional_properties = d
        return ref_role_spec

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
