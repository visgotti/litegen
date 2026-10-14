from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="GatewaySettings")


@_attrs_define
class GatewaySettings:
    """Gateway-wide settings editable at runtime.

    Attributes:
        alert_webhook_url (Union[None, Unset, str]): URL that receives budget alerts (soft budget crossed, hard budget
            hit).
        cost_markup_percent (Union[None, Unset, float]): Percent added on top of provider cost when billing. Overrides
            the config value.
        default_end_user_budget_duration (Union[None, Unset, str]): Window for that default end-user budget.
        default_end_user_max_budget_usd (Union[None, Unset, float]): Default hard budget (USD) for an end user with no
            limits of its own.
    """

    alert_webhook_url: Union[None, Unset, str] = UNSET
    cost_markup_percent: Union[None, Unset, float] = UNSET
    default_end_user_budget_duration: Union[None, Unset, str] = UNSET
    default_end_user_max_budget_usd: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        alert_webhook_url: Union[None, Unset, str]
        if isinstance(self.alert_webhook_url, Unset):
            alert_webhook_url = UNSET
        else:
            alert_webhook_url = self.alert_webhook_url

        cost_markup_percent: Union[None, Unset, float]
        if isinstance(self.cost_markup_percent, Unset):
            cost_markup_percent = UNSET
        else:
            cost_markup_percent = self.cost_markup_percent

        default_end_user_budget_duration: Union[None, Unset, str]
        if isinstance(self.default_end_user_budget_duration, Unset):
            default_end_user_budget_duration = UNSET
        else:
            default_end_user_budget_duration = self.default_end_user_budget_duration

        default_end_user_max_budget_usd: Union[None, Unset, float]
        if isinstance(self.default_end_user_max_budget_usd, Unset):
            default_end_user_max_budget_usd = UNSET
        else:
            default_end_user_max_budget_usd = self.default_end_user_max_budget_usd

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if alert_webhook_url is not UNSET:
            field_dict["alert_webhook_url"] = alert_webhook_url
        if cost_markup_percent is not UNSET:
            field_dict["cost_markup_percent"] = cost_markup_percent
        if default_end_user_budget_duration is not UNSET:
            field_dict["default_end_user_budget_duration"] = (
                default_end_user_budget_duration
            )
        if default_end_user_max_budget_usd is not UNSET:
            field_dict["default_end_user_max_budget_usd"] = (
                default_end_user_max_budget_usd
            )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_alert_webhook_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        alert_webhook_url = _parse_alert_webhook_url(d.pop("alert_webhook_url", UNSET))

        def _parse_cost_markup_percent(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        cost_markup_percent = _parse_cost_markup_percent(
            d.pop("cost_markup_percent", UNSET)
        )

        def _parse_default_end_user_budget_duration(
            data: object,
        ) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        default_end_user_budget_duration = _parse_default_end_user_budget_duration(
            d.pop("default_end_user_budget_duration", UNSET)
        )

        def _parse_default_end_user_max_budget_usd(
            data: object,
        ) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        default_end_user_max_budget_usd = _parse_default_end_user_max_budget_usd(
            d.pop("default_end_user_max_budget_usd", UNSET)
        )

        gateway_settings = cls(
            alert_webhook_url=alert_webhook_url,
            cost_markup_percent=cost_markup_percent,
            default_end_user_budget_duration=default_end_user_budget_duration,
            default_end_user_max_budget_usd=default_end_user_max_budget_usd,
        )

        gateway_settings.additional_properties = d
        return gateway_settings

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
