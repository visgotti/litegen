from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.access_settings import AccessSettings
    from ..models.effective_breaker import EffectiveBreaker
    from ..models.effective_cache import EffectiveCache
    from ..models.effective_retries import EffectiveRetries
    from ..models.gateway_settings import GatewaySettings


T = TypeVar("T", bound="GatewaySettingsView")


@_attrs_define
class GatewaySettingsView:
    """Response for `GET /v1/admin/settings`: the stored overrides plus the
    values in effect.

        Attributes:
            cache_entries (int): Answers the cache holds now (on this replica).
            effective_cache (EffectiveCache): The cache in force (`GET /v1/admin/settings`).
            effective_circuit_breaker (EffectiveBreaker): The circuit breaker in force (`GET /v1/admin/settings`).
            effective_cost_markup_percent (float):
            effective_retries (EffectiveRetries): Retries in force (`GET /v1/admin/settings`).
            effective_storage_access (AccessSettings): The stored-output settings in force.
            effective_vendor_timeout_seconds (int): The vendor call timeout in force, in seconds.
            settings (GatewaySettings): Gateway-wide settings editable at runtime.
    """

    cache_entries: int
    effective_cache: "EffectiveCache"
    effective_circuit_breaker: "EffectiveBreaker"
    effective_cost_markup_percent: float
    effective_retries: "EffectiveRetries"
    effective_storage_access: "AccessSettings"
    effective_vendor_timeout_seconds: int
    settings: "GatewaySettings"
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cache_entries = self.cache_entries

        effective_cache = self.effective_cache.to_dict()

        effective_circuit_breaker = self.effective_circuit_breaker.to_dict()

        effective_cost_markup_percent = self.effective_cost_markup_percent

        effective_retries = self.effective_retries.to_dict()

        effective_storage_access = self.effective_storage_access.to_dict()

        effective_vendor_timeout_seconds = self.effective_vendor_timeout_seconds

        settings = self.settings.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cache_entries": cache_entries,
                "effective_cache": effective_cache,
                "effective_circuit_breaker": effective_circuit_breaker,
                "effective_cost_markup_percent": effective_cost_markup_percent,
                "effective_retries": effective_retries,
                "effective_storage_access": effective_storage_access,
                "effective_vendor_timeout_seconds": effective_vendor_timeout_seconds,
                "settings": settings,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.access_settings import AccessSettings
        from ..models.effective_breaker import EffectiveBreaker
        from ..models.effective_cache import EffectiveCache
        from ..models.effective_retries import EffectiveRetries
        from ..models.gateway_settings import GatewaySettings

        d = src_dict.copy()
        cache_entries = d.pop("cache_entries")

        effective_cache = EffectiveCache.from_dict(d.pop("effective_cache"))

        effective_circuit_breaker = EffectiveBreaker.from_dict(
            d.pop("effective_circuit_breaker")
        )

        effective_cost_markup_percent = d.pop("effective_cost_markup_percent")

        effective_retries = EffectiveRetries.from_dict(d.pop("effective_retries"))

        effective_storage_access = AccessSettings.from_dict(
            d.pop("effective_storage_access")
        )

        effective_vendor_timeout_seconds = d.pop("effective_vendor_timeout_seconds")

        settings = GatewaySettings.from_dict(d.pop("settings"))

        gateway_settings_view = cls(
            cache_entries=cache_entries,
            effective_cache=effective_cache,
            effective_circuit_breaker=effective_circuit_breaker,
            effective_cost_markup_percent=effective_cost_markup_percent,
            effective_retries=effective_retries,
            effective_storage_access=effective_storage_access,
            effective_vendor_timeout_seconds=effective_vendor_timeout_seconds,
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
