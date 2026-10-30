from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.price_rule import PriceRule


T = TypeVar("T", bound="CapabilityModelPricing")


@_attrs_define
class CapabilityModelPricing:
    """
    Attributes:
        base_cost_usd (float): The vendor's price for the model's default request.
        variable_pricing (Union[Unset, List['PriceRule']]): How the price follows the request (3D doc §3G, L14): rules
            applied in
            order to a running price that starts at `base_cost_usd`. The quote,
            the quota reservation and the charge all come from them.
    """

    base_cost_usd: float
    variable_pricing: Union[Unset, List["PriceRule"]] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        base_cost_usd = self.base_cost_usd

        variable_pricing: Union[Unset, List[Dict[str, Any]]] = UNSET
        if not isinstance(self.variable_pricing, Unset):
            variable_pricing = []
            for variable_pricing_item_data in self.variable_pricing:
                variable_pricing_item = variable_pricing_item_data.to_dict()
                variable_pricing.append(variable_pricing_item)

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
        from ..models.price_rule import PriceRule

        d = src_dict.copy()
        base_cost_usd = d.pop("base_cost_usd")

        variable_pricing = []
        _variable_pricing = d.pop("variable_pricing", UNSET)
        for variable_pricing_item_data in _variable_pricing or []:
            variable_pricing_item = PriceRule.from_dict(variable_pricing_item_data)

            variable_pricing.append(variable_pricing_item)

        capability_model_pricing = cls(
            base_cost_usd=base_cost_usd,
            variable_pricing=variable_pricing,
        )

        capability_model_pricing.additional_properties = d
        return capability_model_pricing

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
