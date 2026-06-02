from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.cost_source import CostSource
from ..types import UNSET, Unset

T = TypeVar("T", bound="CostEstimate")


@_attrs_define
class CostEstimate:
    """Cost estimate returned before generation.

    Attributes:
        base_cost_usd (float): Base cost from the provider in USD.
        cost_source (CostSource):
        markup_usd (float): Markup applied (configurable, default 0%).
        tokens_required (int): Equivalent token cost.
        total_cost_usd (float): Total cost including markup.
        breakdown (Union[Unset, Any]): Breakdown details.
    """

    base_cost_usd: float
    cost_source: CostSource
    markup_usd: float
    tokens_required: int
    total_cost_usd: float
    breakdown: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        base_cost_usd = self.base_cost_usd

        cost_source = self.cost_source.value

        markup_usd = self.markup_usd

        tokens_required = self.tokens_required

        total_cost_usd = self.total_cost_usd

        breakdown = self.breakdown

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "base_cost_usd": base_cost_usd,
                "cost_source": cost_source,
                "markup_usd": markup_usd,
                "tokens_required": tokens_required,
                "total_cost_usd": total_cost_usd,
            }
        )
        if breakdown is not UNSET:
            field_dict["breakdown"] = breakdown

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        base_cost_usd = d.pop("base_cost_usd")

        cost_source = CostSource(d.pop("cost_source"))

        markup_usd = d.pop("markup_usd")

        tokens_required = d.pop("tokens_required")

        total_cost_usd = d.pop("total_cost_usd")

        breakdown = d.pop("breakdown", UNSET)

        cost_estimate = cls(
            base_cost_usd=base_cost_usd,
            cost_source=cost_source,
            markup_usd=markup_usd,
            tokens_required=tokens_required,
            total_cost_usd=total_cost_usd,
            breakdown=breakdown,
        )

        cost_estimate.additional_properties = d
        return cost_estimate

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
