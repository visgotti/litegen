from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ModelSchemaExtraParams")


@_attrs_define
class ModelSchemaExtraParams:
    """The vendor's type for an `extra_allowlist` key, by key: `bool`,
    `int`, `float`, `string` or `string_array`, with its bounds or values
    when they are known. A request's value is checked like a param's
    (`param: "extra.<key>"`); `null` passes as sent, and an allowlisted key
    with no type here passes as any JSON.

    """

    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        model_schema_extra_params = cls()

        model_schema_extra_params.additional_properties = d
        return model_schema_extra_params

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
