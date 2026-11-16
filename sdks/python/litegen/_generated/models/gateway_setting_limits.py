from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.setting_range import SettingRange


T = TypeVar("T", bound="GatewaySettingLimits")


@_attrs_define
class GatewaySettingLimits:
    """The range of each numeric gateway setting, named as the setting is: what
    `PUT /v1/admin/settings` holds it to, served so a client need not keep a
    copy. `default_end_user_max_budget_usd` has no upper bound, so it has none.

        Attributes:
            cache_max_bytes (SettingRange): The lowest and the highest value a setting takes, both allowed.
            cache_max_items (SettingRange): The lowest and the highest value a setting takes, both allowed.
            cache_ttl_seconds (SettingRange): The lowest and the highest value a setting takes, both allowed.
            circuit_breaker_open_for_seconds (SettingRange): The lowest and the highest value a setting takes, both allowed.
            circuit_breaker_threshold (SettingRange): The lowest and the highest value a setting takes, both allowed.
            cost_markup_percent (SettingRange): The lowest and the highest value a setting takes, both allowed.
            retry_backoff_ms (SettingRange): The lowest and the highest value a setting takes, both allowed.
            retry_max_backoff_ms (SettingRange): The lowest and the highest value a setting takes, both allowed.
            retry_max_retries (SettingRange): The lowest and the highest value a setting takes, both allowed.
            storage_access_retention_days (SettingRange): The lowest and the highest value a setting takes, both allowed.
            storage_access_signed_url_ttl_seconds (SettingRange): The lowest and the highest value a setting takes, both
                allowed.
            vendor_timeout_seconds (SettingRange): The lowest and the highest value a setting takes, both allowed.
    """

    cache_max_bytes: "SettingRange"
    cache_max_items: "SettingRange"
    cache_ttl_seconds: "SettingRange"
    circuit_breaker_open_for_seconds: "SettingRange"
    circuit_breaker_threshold: "SettingRange"
    cost_markup_percent: "SettingRange"
    retry_backoff_ms: "SettingRange"
    retry_max_backoff_ms: "SettingRange"
    retry_max_retries: "SettingRange"
    storage_access_retention_days: "SettingRange"
    storage_access_signed_url_ttl_seconds: "SettingRange"
    vendor_timeout_seconds: "SettingRange"
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cache_max_bytes = self.cache_max_bytes.to_dict()

        cache_max_items = self.cache_max_items.to_dict()

        cache_ttl_seconds = self.cache_ttl_seconds.to_dict()

        circuit_breaker_open_for_seconds = (
            self.circuit_breaker_open_for_seconds.to_dict()
        )

        circuit_breaker_threshold = self.circuit_breaker_threshold.to_dict()

        cost_markup_percent = self.cost_markup_percent.to_dict()

        retry_backoff_ms = self.retry_backoff_ms.to_dict()

        retry_max_backoff_ms = self.retry_max_backoff_ms.to_dict()

        retry_max_retries = self.retry_max_retries.to_dict()

        storage_access_retention_days = self.storage_access_retention_days.to_dict()

        storage_access_signed_url_ttl_seconds = (
            self.storage_access_signed_url_ttl_seconds.to_dict()
        )

        vendor_timeout_seconds = self.vendor_timeout_seconds.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cache_max_bytes": cache_max_bytes,
                "cache_max_items": cache_max_items,
                "cache_ttl_seconds": cache_ttl_seconds,
                "circuit_breaker_open_for_seconds": circuit_breaker_open_for_seconds,
                "circuit_breaker_threshold": circuit_breaker_threshold,
                "cost_markup_percent": cost_markup_percent,
                "retry_backoff_ms": retry_backoff_ms,
                "retry_max_backoff_ms": retry_max_backoff_ms,
                "retry_max_retries": retry_max_retries,
                "storage_access_retention_days": storage_access_retention_days,
                "storage_access_signed_url_ttl_seconds": storage_access_signed_url_ttl_seconds,
                "vendor_timeout_seconds": vendor_timeout_seconds,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.setting_range import SettingRange

        d = src_dict.copy()
        cache_max_bytes = SettingRange.from_dict(d.pop("cache_max_bytes"))

        cache_max_items = SettingRange.from_dict(d.pop("cache_max_items"))

        cache_ttl_seconds = SettingRange.from_dict(d.pop("cache_ttl_seconds"))

        circuit_breaker_open_for_seconds = SettingRange.from_dict(
            d.pop("circuit_breaker_open_for_seconds")
        )

        circuit_breaker_threshold = SettingRange.from_dict(
            d.pop("circuit_breaker_threshold")
        )

        cost_markup_percent = SettingRange.from_dict(d.pop("cost_markup_percent"))

        retry_backoff_ms = SettingRange.from_dict(d.pop("retry_backoff_ms"))

        retry_max_backoff_ms = SettingRange.from_dict(d.pop("retry_max_backoff_ms"))

        retry_max_retries = SettingRange.from_dict(d.pop("retry_max_retries"))

        storage_access_retention_days = SettingRange.from_dict(
            d.pop("storage_access_retention_days")
        )

        storage_access_signed_url_ttl_seconds = SettingRange.from_dict(
            d.pop("storage_access_signed_url_ttl_seconds")
        )

        vendor_timeout_seconds = SettingRange.from_dict(d.pop("vendor_timeout_seconds"))

        gateway_setting_limits = cls(
            cache_max_bytes=cache_max_bytes,
            cache_max_items=cache_max_items,
            cache_ttl_seconds=cache_ttl_seconds,
            circuit_breaker_open_for_seconds=circuit_breaker_open_for_seconds,
            circuit_breaker_threshold=circuit_breaker_threshold,
            cost_markup_percent=cost_markup_percent,
            retry_backoff_ms=retry_backoff_ms,
            retry_max_backoff_ms=retry_max_backoff_ms,
            retry_max_retries=retry_max_retries,
            storage_access_retention_days=storage_access_retention_days,
            storage_access_signed_url_ttl_seconds=storage_access_signed_url_ttl_seconds,
            vendor_timeout_seconds=vendor_timeout_seconds,
        )

        gateway_setting_limits.additional_properties = d
        return gateway_setting_limits

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
