from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.cost_source import CostSource

T = TypeVar("T", bound="UsageInfo")


@_attrs_define
class UsageInfo:
    """Cost / usage information for a generation.

    Attributes:
        cost_source (CostSource):
        cost_usd (float): Provider cost in USD.
        tokens (int): Internal token cost.
    """

    cost_source: CostSource
    cost_usd: float
    tokens: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cost_source = self.cost_source.value

        cost_usd = self.cost_usd

        tokens = self.tokens

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cost_source": cost_source,
                "cost_usd": cost_usd,
                "tokens": tokens,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        cost_source = CostSource(d.pop("cost_source"))

        cost_usd = d.pop("cost_usd")

        tokens = d.pop("tokens")

        usage_info = cls(
            cost_source=cost_source,
            cost_usd=cost_usd,
            tokens=tokens,
        )

        usage_info.additional_properties = d
        return usage_info

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
