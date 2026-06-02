from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ProviderUsageStat")


@_attrs_define
class ProviderUsageStat:
    """
    Attributes:
        avg_latency_ms (float):
        cost_usd (float):
        failures (int):
        provider (str):
        requests (int):
    """

    avg_latency_ms: float
    cost_usd: float
    failures: int
    provider: str
    requests: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        avg_latency_ms = self.avg_latency_ms

        cost_usd = self.cost_usd

        failures = self.failures

        provider = self.provider

        requests = self.requests

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "avg_latency_ms": avg_latency_ms,
                "cost_usd": cost_usd,
                "failures": failures,
                "provider": provider,
                "requests": requests,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        avg_latency_ms = d.pop("avg_latency_ms")

        cost_usd = d.pop("cost_usd")

        failures = d.pop("failures")

        provider = d.pop("provider")

        requests = d.pop("requests")

        provider_usage_stat = cls(
            avg_latency_ms=avg_latency_ms,
            cost_usd=cost_usd,
            failures=failures,
            provider=provider,
            requests=requests,
        )

        provider_usage_stat.additional_properties = d
        return provider_usage_stat

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
