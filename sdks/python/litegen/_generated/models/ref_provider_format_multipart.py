from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.ref_provider_format_multipart_field_map import (
        RefProviderFormatMultipartFieldMap,
    )


T = TypeVar("T", bound="RefProviderFormatMultipart")


@_attrs_define
class RefProviderFormatMultipart:
    """
    Attributes:
        field_map (RefProviderFormatMultipartFieldMap):
    """

    field_map: "RefProviderFormatMultipartFieldMap"
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        field_map = self.field_map.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "field_map": field_map,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.ref_provider_format_multipart_field_map import (
            RefProviderFormatMultipartFieldMap,
        )

        d = src_dict.copy()
        field_map = RefProviderFormatMultipartFieldMap.from_dict(d.pop("field_map"))

        ref_provider_format_multipart = cls(
            field_map=field_map,
        )

        ref_provider_format_multipart.additional_properties = d
        return ref_provider_format_multipart

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
