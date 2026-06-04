from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="LatencyPercentiles")


@_attrs_define
class LatencyPercentiles:
    """Latency percentiles for the last N minutes.

    Attributes:
        p50_ms (float):
        p95_ms (float):
        p99_ms (float):
        sample_count (int):
        window_minutes (int):
    """

    p50_ms: float
    p95_ms: float
    p99_ms: float
    sample_count: int
    window_minutes: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        p50_ms = self.p50_ms

        p95_ms = self.p95_ms

        p99_ms = self.p99_ms

        sample_count = self.sample_count

        window_minutes = self.window_minutes

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "p50_ms": p50_ms,
                "p95_ms": p95_ms,
                "p99_ms": p99_ms,
                "sample_count": sample_count,
                "window_minutes": window_minutes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        p50_ms = d.pop("p50_ms")

        p95_ms = d.pop("p95_ms")

        p99_ms = d.pop("p99_ms")

        sample_count = d.pop("sample_count")

        window_minutes = d.pop("window_minutes")

        latency_percentiles = cls(
            p50_ms=p50_ms,
            p95_ms=p95_ms,
            p99_ms=p99_ms,
            sample_count=sample_count,
            window_minutes=window_minutes,
        )

        latency_percentiles.additional_properties = d
        return latency_percentiles

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
