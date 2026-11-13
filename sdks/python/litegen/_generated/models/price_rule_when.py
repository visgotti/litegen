from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PriceRuleWhen")


@_attrs_define
class PriceRuleWhen:
    """Request facts the rule needs: a param (as sent, else its default), an
    `extra.<field>`, or a derived fact (the `views`, `added_views`,
    `keyframes`, `animations` and `references` counts, `duration_seconds`,
    `megapixels`), each mapped to the value it must have, `"*"` for any
    value present, or a range a number must fall in, such as
    `{"gt": 1.048576, "lte": 4.194304}` (`gt`, `gte`, `lt`, `lte`).

    """

    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        price_rule_when = cls()

        price_rule_when.additional_properties = d
        return price_rule_when

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
