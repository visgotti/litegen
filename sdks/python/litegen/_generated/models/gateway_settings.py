from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.access_mode import AccessMode
from ..models.owner_scope import OwnerScope
from ..models.signed_links import SignedLinks
from ..types import UNSET, Unset

T = TypeVar("T", bound="GatewaySettings")


@_attrs_define
class GatewaySettings:
    """Gateway-wide settings editable at runtime.

    Attributes:
        alert_webhook_url (Union[None, Unset, str]): URL that receives budget alerts (soft budget crossed, hard budget
            hit).
        cache_enabled (Union[None, Unset, bool]): Whether image answers are cached. Overrides `cache.enabled`.
        cache_max_items (Union[None, Unset, int]): How many answers the cache holds, 1 to 1000000. Overrides
            `cache.max_items`.
        cache_ttl_seconds (Union[None, Unset, int]): How long a cached answer lives, 1 to 604800 s. Overrides
            `cache.default_ttl_seconds`.
        circuit_breaker_open_for_seconds (Union[None, Unset, int]): Seconds an open breaker stays open, 1 to 3600.
            Overrides `circuit_breaker.open_for_seconds`.
        circuit_breaker_threshold (Union[None, Unset, int]): Consecutive failures that open a provider's breaker, 1 to
            1000. Overrides `circuit_breaker.threshold`.
        cost_markup_percent (Union[None, Unset, float]): Percent added on top of provider cost when billing. Overrides
            the config value.
        default_end_user_budget_duration (Union[None, Unset, str]): Window for that default end-user budget.
        default_end_user_max_budget_usd (Union[None, Unset, float]): Default hard budget (USD) for an end user with no
            limits of its own.
        retry_backoff_ms (Union[None, Unset, int]): The longest first wait before a retry, 0 to 60000 ms. Overrides
            `retries.backoff_ms`.
        retry_max_backoff_ms (Union[None, Unset, int]): The longest any wait may be, up to 60000 ms. Overrides
            `retries.max_backoff_ms`.
        retry_max_retries (Union[None, Unset, int]): Tries after the first for a retryable vendor error, 0 to 5.
            Overrides `retries.max_retries`.
        storage_access_keep_outputs (Union[None, Unset, bool]): Whether litegen keeps a copy of each output. Overrides
            `storage_access.keep_outputs`.
        storage_access_mode (Union[AccessMode, None, Unset]):
        storage_access_owner_scope (Union[None, OwnerScope, Unset]):
        storage_access_retention_days (Union[None, Unset, int]): Days after which kept files are deleted, 0 to 36500 (0:
            until deleted).
            Overrides `storage_access.retention_days`.
        storage_access_signed_links (Union[None, SignedLinks, Unset]):
        storage_access_signed_url_ttl_seconds (Union[None, Unset, int]): How long a signed link works, 60 to 604800
            seconds. Overrides
            `storage_access.signed_url_ttl_seconds`.
        vendor_timeout_seconds (Union[None, Unset, int]): Seconds one vendor call may take, 5 to 3600. Overrides
            `server.vendor_timeout_secs`.
    """

    alert_webhook_url: Union[None, Unset, str] = UNSET
    cache_enabled: Union[None, Unset, bool] = UNSET
    cache_max_items: Union[None, Unset, int] = UNSET
    cache_ttl_seconds: Union[None, Unset, int] = UNSET
    circuit_breaker_open_for_seconds: Union[None, Unset, int] = UNSET
    circuit_breaker_threshold: Union[None, Unset, int] = UNSET
    cost_markup_percent: Union[None, Unset, float] = UNSET
    default_end_user_budget_duration: Union[None, Unset, str] = UNSET
    default_end_user_max_budget_usd: Union[None, Unset, float] = UNSET
    retry_backoff_ms: Union[None, Unset, int] = UNSET
    retry_max_backoff_ms: Union[None, Unset, int] = UNSET
    retry_max_retries: Union[None, Unset, int] = UNSET
    storage_access_keep_outputs: Union[None, Unset, bool] = UNSET
    storage_access_mode: Union[AccessMode, None, Unset] = UNSET
    storage_access_owner_scope: Union[None, OwnerScope, Unset] = UNSET
    storage_access_retention_days: Union[None, Unset, int] = UNSET
    storage_access_signed_links: Union[None, SignedLinks, Unset] = UNSET
    storage_access_signed_url_ttl_seconds: Union[None, Unset, int] = UNSET
    vendor_timeout_seconds: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        alert_webhook_url: Union[None, Unset, str]
        if isinstance(self.alert_webhook_url, Unset):
            alert_webhook_url = UNSET
        else:
            alert_webhook_url = self.alert_webhook_url

        cache_enabled: Union[None, Unset, bool]
        if isinstance(self.cache_enabled, Unset):
            cache_enabled = UNSET
        else:
            cache_enabled = self.cache_enabled

        cache_max_items: Union[None, Unset, int]
        if isinstance(self.cache_max_items, Unset):
            cache_max_items = UNSET
        else:
            cache_max_items = self.cache_max_items

        cache_ttl_seconds: Union[None, Unset, int]
        if isinstance(self.cache_ttl_seconds, Unset):
            cache_ttl_seconds = UNSET
        else:
            cache_ttl_seconds = self.cache_ttl_seconds

        circuit_breaker_open_for_seconds: Union[None, Unset, int]
        if isinstance(self.circuit_breaker_open_for_seconds, Unset):
            circuit_breaker_open_for_seconds = UNSET
        else:
            circuit_breaker_open_for_seconds = self.circuit_breaker_open_for_seconds

        circuit_breaker_threshold: Union[None, Unset, int]
        if isinstance(self.circuit_breaker_threshold, Unset):
            circuit_breaker_threshold = UNSET
        else:
            circuit_breaker_threshold = self.circuit_breaker_threshold

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

        retry_backoff_ms: Union[None, Unset, int]
        if isinstance(self.retry_backoff_ms, Unset):
            retry_backoff_ms = UNSET
        else:
            retry_backoff_ms = self.retry_backoff_ms

        retry_max_backoff_ms: Union[None, Unset, int]
        if isinstance(self.retry_max_backoff_ms, Unset):
            retry_max_backoff_ms = UNSET
        else:
            retry_max_backoff_ms = self.retry_max_backoff_ms

        retry_max_retries: Union[None, Unset, int]
        if isinstance(self.retry_max_retries, Unset):
            retry_max_retries = UNSET
        else:
            retry_max_retries = self.retry_max_retries

        storage_access_keep_outputs: Union[None, Unset, bool]
        if isinstance(self.storage_access_keep_outputs, Unset):
            storage_access_keep_outputs = UNSET
        else:
            storage_access_keep_outputs = self.storage_access_keep_outputs

        storage_access_mode: Union[None, Unset, str]
        if isinstance(self.storage_access_mode, Unset):
            storage_access_mode = UNSET
        elif isinstance(self.storage_access_mode, AccessMode):
            storage_access_mode = self.storage_access_mode.value
        else:
            storage_access_mode = self.storage_access_mode

        storage_access_owner_scope: Union[None, Unset, str]
        if isinstance(self.storage_access_owner_scope, Unset):
            storage_access_owner_scope = UNSET
        elif isinstance(self.storage_access_owner_scope, OwnerScope):
            storage_access_owner_scope = self.storage_access_owner_scope.value
        else:
            storage_access_owner_scope = self.storage_access_owner_scope

        storage_access_retention_days: Union[None, Unset, int]
        if isinstance(self.storage_access_retention_days, Unset):
            storage_access_retention_days = UNSET
        else:
            storage_access_retention_days = self.storage_access_retention_days

        storage_access_signed_links: Union[None, Unset, str]
        if isinstance(self.storage_access_signed_links, Unset):
            storage_access_signed_links = UNSET
        elif isinstance(self.storage_access_signed_links, SignedLinks):
            storage_access_signed_links = self.storage_access_signed_links.value
        else:
            storage_access_signed_links = self.storage_access_signed_links

        storage_access_signed_url_ttl_seconds: Union[None, Unset, int]
        if isinstance(self.storage_access_signed_url_ttl_seconds, Unset):
            storage_access_signed_url_ttl_seconds = UNSET
        else:
            storage_access_signed_url_ttl_seconds = (
                self.storage_access_signed_url_ttl_seconds
            )

        vendor_timeout_seconds: Union[None, Unset, int]
        if isinstance(self.vendor_timeout_seconds, Unset):
            vendor_timeout_seconds = UNSET
        else:
            vendor_timeout_seconds = self.vendor_timeout_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if alert_webhook_url is not UNSET:
            field_dict["alert_webhook_url"] = alert_webhook_url
        if cache_enabled is not UNSET:
            field_dict["cache_enabled"] = cache_enabled
        if cache_max_items is not UNSET:
            field_dict["cache_max_items"] = cache_max_items
        if cache_ttl_seconds is not UNSET:
            field_dict["cache_ttl_seconds"] = cache_ttl_seconds
        if circuit_breaker_open_for_seconds is not UNSET:
            field_dict["circuit_breaker_open_for_seconds"] = (
                circuit_breaker_open_for_seconds
            )
        if circuit_breaker_threshold is not UNSET:
            field_dict["circuit_breaker_threshold"] = circuit_breaker_threshold
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
        if retry_backoff_ms is not UNSET:
            field_dict["retry_backoff_ms"] = retry_backoff_ms
        if retry_max_backoff_ms is not UNSET:
            field_dict["retry_max_backoff_ms"] = retry_max_backoff_ms
        if retry_max_retries is not UNSET:
            field_dict["retry_max_retries"] = retry_max_retries
        if storage_access_keep_outputs is not UNSET:
            field_dict["storage_access_keep_outputs"] = storage_access_keep_outputs
        if storage_access_mode is not UNSET:
            field_dict["storage_access_mode"] = storage_access_mode
        if storage_access_owner_scope is not UNSET:
            field_dict["storage_access_owner_scope"] = storage_access_owner_scope
        if storage_access_retention_days is not UNSET:
            field_dict["storage_access_retention_days"] = storage_access_retention_days
        if storage_access_signed_links is not UNSET:
            field_dict["storage_access_signed_links"] = storage_access_signed_links
        if storage_access_signed_url_ttl_seconds is not UNSET:
            field_dict["storage_access_signed_url_ttl_seconds"] = (
                storage_access_signed_url_ttl_seconds
            )
        if vendor_timeout_seconds is not UNSET:
            field_dict["vendor_timeout_seconds"] = vendor_timeout_seconds

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

        def _parse_cache_enabled(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        cache_enabled = _parse_cache_enabled(d.pop("cache_enabled", UNSET))

        def _parse_cache_max_items(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        cache_max_items = _parse_cache_max_items(d.pop("cache_max_items", UNSET))

        def _parse_cache_ttl_seconds(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        cache_ttl_seconds = _parse_cache_ttl_seconds(d.pop("cache_ttl_seconds", UNSET))

        def _parse_circuit_breaker_open_for_seconds(
            data: object,
        ) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        circuit_breaker_open_for_seconds = _parse_circuit_breaker_open_for_seconds(
            d.pop("circuit_breaker_open_for_seconds", UNSET)
        )

        def _parse_circuit_breaker_threshold(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        circuit_breaker_threshold = _parse_circuit_breaker_threshold(
            d.pop("circuit_breaker_threshold", UNSET)
        )

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

        def _parse_retry_backoff_ms(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        retry_backoff_ms = _parse_retry_backoff_ms(d.pop("retry_backoff_ms", UNSET))

        def _parse_retry_max_backoff_ms(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        retry_max_backoff_ms = _parse_retry_max_backoff_ms(
            d.pop("retry_max_backoff_ms", UNSET)
        )

        def _parse_retry_max_retries(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        retry_max_retries = _parse_retry_max_retries(d.pop("retry_max_retries", UNSET))

        def _parse_storage_access_keep_outputs(
            data: object,
        ) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        storage_access_keep_outputs = _parse_storage_access_keep_outputs(
            d.pop("storage_access_keep_outputs", UNSET)
        )

        def _parse_storage_access_mode(data: object) -> Union[AccessMode, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                storage_access_mode_type_1 = AccessMode(data)

                return storage_access_mode_type_1
            except:  # noqa: E722
                pass
            return cast(Union[AccessMode, None, Unset], data)

        storage_access_mode = _parse_storage_access_mode(
            d.pop("storage_access_mode", UNSET)
        )

        def _parse_storage_access_owner_scope(
            data: object,
        ) -> Union[None, OwnerScope, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                storage_access_owner_scope_type_1 = OwnerScope(data)

                return storage_access_owner_scope_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, OwnerScope, Unset], data)

        storage_access_owner_scope = _parse_storage_access_owner_scope(
            d.pop("storage_access_owner_scope", UNSET)
        )

        def _parse_storage_access_retention_days(
            data: object,
        ) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        storage_access_retention_days = _parse_storage_access_retention_days(
            d.pop("storage_access_retention_days", UNSET)
        )

        def _parse_storage_access_signed_links(
            data: object,
        ) -> Union[None, SignedLinks, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                storage_access_signed_links_type_1 = SignedLinks(data)

                return storage_access_signed_links_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, SignedLinks, Unset], data)

        storage_access_signed_links = _parse_storage_access_signed_links(
            d.pop("storage_access_signed_links", UNSET)
        )

        def _parse_storage_access_signed_url_ttl_seconds(
            data: object,
        ) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        storage_access_signed_url_ttl_seconds = (
            _parse_storage_access_signed_url_ttl_seconds(
                d.pop("storage_access_signed_url_ttl_seconds", UNSET)
            )
        )

        def _parse_vendor_timeout_seconds(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        vendor_timeout_seconds = _parse_vendor_timeout_seconds(
            d.pop("vendor_timeout_seconds", UNSET)
        )

        gateway_settings = cls(
            alert_webhook_url=alert_webhook_url,
            cache_enabled=cache_enabled,
            cache_max_items=cache_max_items,
            cache_ttl_seconds=cache_ttl_seconds,
            circuit_breaker_open_for_seconds=circuit_breaker_open_for_seconds,
            circuit_breaker_threshold=circuit_breaker_threshold,
            cost_markup_percent=cost_markup_percent,
            default_end_user_budget_duration=default_end_user_budget_duration,
            default_end_user_max_budget_usd=default_end_user_max_budget_usd,
            retry_backoff_ms=retry_backoff_ms,
            retry_max_backoff_ms=retry_max_backoff_ms,
            retry_max_retries=retry_max_retries,
            storage_access_keep_outputs=storage_access_keep_outputs,
            storage_access_mode=storage_access_mode,
            storage_access_owner_scope=storage_access_owner_scope,
            storage_access_retention_days=storage_access_retention_days,
            storage_access_signed_links=storage_access_signed_links,
            storage_access_signed_url_ttl_seconds=storage_access_signed_url_ttl_seconds,
            vendor_timeout_seconds=vendor_timeout_seconds,
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
