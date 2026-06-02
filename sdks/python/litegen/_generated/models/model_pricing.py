from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ModelPricing")


@_attrs_define
class ModelPricing:
    """
    Attributes:
        base_cost_usd (float): Base cost per image/video in USD.
        variable_pricing (Union[Unset, Any]): Variable pricing by dimension (JSON map).
    """

    base_cost_usd: float
    variable_pricing: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        base_cost_usd = self.base_cost_usd

        variable_pricing = self.variable_pricing

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "base_cost_usd": base_cost_usd,
            }
        )
        if variable_pricing is not UNSET:
            field_dict["variable_pricing"] = variable_pricing

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        base_cost_usd = d.pop("base_cost_usd")

        variable_pricing = d.pop("variable_pricing", UNSET)

        model_pricing = cls(
            base_cost_usd=base_cost_usd,
            variable_pricing=variable_pricing,
        )

        model_pricing.additional_properties = d
        return model_pricing

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
