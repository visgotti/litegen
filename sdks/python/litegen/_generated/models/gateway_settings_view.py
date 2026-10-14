from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.gateway_settings import GatewaySettings


T = TypeVar("T", bound="GatewaySettingsView")


@_attrs_define
class GatewaySettingsView:
    """Response for `GET /v1/admin/settings`: the stored overrides plus the
    values in effect.

        Attributes:
            effective_cost_markup_percent (float):
            settings (GatewaySettings): Gateway-wide settings editable at runtime.
    """

    effective_cost_markup_percent: float
    settings: "GatewaySettings"
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        effective_cost_markup_percent = self.effective_cost_markup_percent

        settings = self.settings.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "effective_cost_markup_percent": effective_cost_markup_percent,
                "settings": settings,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.gateway_settings import GatewaySettings

        d = src_dict.copy()
        effective_cost_markup_percent = d.pop("effective_cost_markup_percent")

        settings = GatewaySettings.from_dict(d.pop("settings"))

        gateway_settings_view = cls(
            effective_cost_markup_percent=effective_cost_markup_percent,
            settings=settings,
        )

        gateway_settings_view.additional_properties = d
        return gateway_settings_view

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
