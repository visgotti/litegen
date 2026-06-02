from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.model_usage_stat import ModelUsageStat
    from ..models.provider_usage_stat import ProviderUsageStat


T = TypeVar("T", bound="ProxyStats")


@_attrs_define
class ProxyStats:
    """Aggregate stats for the dashboard.

    Attributes:
        avg_latency_ms (float):
        failed_requests (int):
        models_used (List['ModelUsageStat']):
        providers_used (List['ProviderUsageStat']):
        requests_per_minute (float):
        successful_requests (int):
        total_cost_usd (float):
        total_requests (int):
    """

    avg_latency_ms: float
    failed_requests: int
    models_used: List["ModelUsageStat"]
    providers_used: List["ProviderUsageStat"]
    requests_per_minute: float
    successful_requests: int
    total_cost_usd: float
    total_requests: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        avg_latency_ms = self.avg_latency_ms

        failed_requests = self.failed_requests

        models_used = []
        for models_used_item_data in self.models_used:
            models_used_item = models_used_item_data.to_dict()
            models_used.append(models_used_item)

        providers_used = []
        for providers_used_item_data in self.providers_used:
            providers_used_item = providers_used_item_data.to_dict()
            providers_used.append(providers_used_item)

        requests_per_minute = self.requests_per_minute

        successful_requests = self.successful_requests

        total_cost_usd = self.total_cost_usd

        total_requests = self.total_requests

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "avg_latency_ms": avg_latency_ms,
                "failed_requests": failed_requests,
                "models_used": models_used,
                "providers_used": providers_used,
                "requests_per_minute": requests_per_minute,
                "successful_requests": successful_requests,
                "total_cost_usd": total_cost_usd,
                "total_requests": total_requests,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.model_usage_stat import ModelUsageStat
        from ..models.provider_usage_stat import ProviderUsageStat

        d = src_dict.copy()
        avg_latency_ms = d.pop("avg_latency_ms")

        failed_requests = d.pop("failed_requests")

        models_used = []
        _models_used = d.pop("models_used")
        for models_used_item_data in _models_used:
            models_used_item = ModelUsageStat.from_dict(models_used_item_data)

            models_used.append(models_used_item)

        providers_used = []
        _providers_used = d.pop("providers_used")
        for providers_used_item_data in _providers_used:
            providers_used_item = ProviderUsageStat.from_dict(providers_used_item_data)

            providers_used.append(providers_used_item)

        requests_per_minute = d.pop("requests_per_minute")

        successful_requests = d.pop("successful_requests")

        total_cost_usd = d.pop("total_cost_usd")

        total_requests = d.pop("total_requests")

        proxy_stats = cls(
            avg_latency_ms=avg_latency_ms,
            failed_requests=failed_requests,
            models_used=models_used,
            providers_used=providers_used,
            requests_per_minute=requests_per_minute,
            successful_requests=successful_requests,
            total_cost_usd=total_cost_usd,
            total_requests=total_requests,
        )

        proxy_stats.additional_properties = d
        return proxy_stats

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
