from typing import TYPE_CHECKING, Any, Dict, Type, TypeVar, Union, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.price_rule_when import PriceRuleWhen


T = TypeVar("T", bound="PriceRule")


@_attrs_define
class PriceRule:
    """One pricing rule. When every `when` entry matches the request, its one
    effect applies: a price (`usd`), a rate (`per_second_usd` × the clip's
    seconds, `per_megapixel_usd` × the image's megapixels, `per_item_usd` × the
    count of the `per_item` list) or an adjustment (`multiply`, `add_usd`).

        Attributes:
            add_usd (Union[None, Unset, float]):
            multiply (Union[None, Unset, float]):
            multiply_per_megapixel_above (Union[None, Unset, float]): Scale the price by the image's `megapixels` over this
                many, for an
                image larger than this: a vendor that bills by output size (OpenAI's
                image tokens) past the size its base price is for.
            per_item (Union[None, Unset, str]): The list `per_item_usd` counts (`animations`, `views`, `keyframes`).
            per_item_usd (Union[None, Unset, float]):
            per_megapixel_usd (Union[None, Unset, float]):
            per_second_usd (Union[None, Unset, float]):
            usd (Union[None, Unset, float]):
            when (Union[Unset, PriceRuleWhen]): Request facts the rule needs: a param (as sent, else its default), an
                `extra.<field>`, or a derived fact (the `views`, `added_views`,
                `keyframes`, `animations` and `references` counts, `duration_seconds`,
                `megapixels`), each mapped to the value it must have, `"*"` for any
                value present, or a range a number must fall in, such as
                `{"gt": 1.048576, "lte": 4.194304}` (`gt`, `gte`, `lt`, `lte`).
    """

    add_usd: Union[None, Unset, float] = UNSET
    multiply: Union[None, Unset, float] = UNSET
    multiply_per_megapixel_above: Union[None, Unset, float] = UNSET
    per_item: Union[None, Unset, str] = UNSET
    per_item_usd: Union[None, Unset, float] = UNSET
    per_megapixel_usd: Union[None, Unset, float] = UNSET
    per_second_usd: Union[None, Unset, float] = UNSET
    usd: Union[None, Unset, float] = UNSET
    when: Union[Unset, "PriceRuleWhen"] = UNSET

    def to_dict(self) -> Dict[str, Any]:
        add_usd: Union[None, Unset, float]
        if isinstance(self.add_usd, Unset):
            add_usd = UNSET
        else:
            add_usd = self.add_usd

        multiply: Union[None, Unset, float]
        if isinstance(self.multiply, Unset):
            multiply = UNSET
        else:
            multiply = self.multiply

        multiply_per_megapixel_above: Union[None, Unset, float]
        if isinstance(self.multiply_per_megapixel_above, Unset):
            multiply_per_megapixel_above = UNSET
        else:
            multiply_per_megapixel_above = self.multiply_per_megapixel_above

        per_item: Union[None, Unset, str]
        if isinstance(self.per_item, Unset):
            per_item = UNSET
        else:
            per_item = self.per_item

        per_item_usd: Union[None, Unset, float]
        if isinstance(self.per_item_usd, Unset):
            per_item_usd = UNSET
        else:
            per_item_usd = self.per_item_usd

        per_megapixel_usd: Union[None, Unset, float]
        if isinstance(self.per_megapixel_usd, Unset):
            per_megapixel_usd = UNSET
        else:
            per_megapixel_usd = self.per_megapixel_usd

        per_second_usd: Union[None, Unset, float]
        if isinstance(self.per_second_usd, Unset):
            per_second_usd = UNSET
        else:
            per_second_usd = self.per_second_usd

        usd: Union[None, Unset, float]
        if isinstance(self.usd, Unset):
            usd = UNSET
        else:
            usd = self.usd

        when: Union[Unset, Dict[str, Any]] = UNSET
        if not isinstance(self.when, Unset):
            when = self.when.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update({})
        if add_usd is not UNSET:
            field_dict["add_usd"] = add_usd
        if multiply is not UNSET:
            field_dict["multiply"] = multiply
        if multiply_per_megapixel_above is not UNSET:
            field_dict["multiply_per_megapixel_above"] = multiply_per_megapixel_above
        if per_item is not UNSET:
            field_dict["per_item"] = per_item
        if per_item_usd is not UNSET:
            field_dict["per_item_usd"] = per_item_usd
        if per_megapixel_usd is not UNSET:
            field_dict["per_megapixel_usd"] = per_megapixel_usd
        if per_second_usd is not UNSET:
            field_dict["per_second_usd"] = per_second_usd
        if usd is not UNSET:
            field_dict["usd"] = usd
        if when is not UNSET:
            field_dict["when"] = when

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.price_rule_when import PriceRuleWhen

        d = src_dict.copy()

        def _parse_add_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        add_usd = _parse_add_usd(d.pop("add_usd", UNSET))

        def _parse_multiply(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        multiply = _parse_multiply(d.pop("multiply", UNSET))

        def _parse_multiply_per_megapixel_above(
            data: object,
        ) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        multiply_per_megapixel_above = _parse_multiply_per_megapixel_above(
            d.pop("multiply_per_megapixel_above", UNSET)
        )

        def _parse_per_item(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        per_item = _parse_per_item(d.pop("per_item", UNSET))

        def _parse_per_item_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        per_item_usd = _parse_per_item_usd(d.pop("per_item_usd", UNSET))

        def _parse_per_megapixel_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        per_megapixel_usd = _parse_per_megapixel_usd(d.pop("per_megapixel_usd", UNSET))

        def _parse_per_second_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        per_second_usd = _parse_per_second_usd(d.pop("per_second_usd", UNSET))

        def _parse_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        usd = _parse_usd(d.pop("usd", UNSET))

        _when = d.pop("when", UNSET)
        when: Union[Unset, PriceRuleWhen]
        if isinstance(_when, Unset):
            when = UNSET
        else:
            when = PriceRuleWhen.from_dict(_when)

        price_rule = cls(
            add_usd=add_usd,
            multiply=multiply,
            multiply_per_megapixel_above=multiply_per_megapixel_above,
            per_item=per_item,
            per_item_usd=per_item_usd,
            per_megapixel_usd=per_megapixel_usd,
            per_second_usd=per_second_usd,
            usd=usd,
            when=when,
        )

        return price_rule
