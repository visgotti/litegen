from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="EffectiveBreaker")


@_attrs_define
class EffectiveBreaker:
    """The circuit breaker in force (`GET /v1/admin/settings`).

    Attributes:
        open_for_seconds (int):
        threshold (int):
    """

    open_for_seconds: int
    threshold: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        open_for_seconds = self.open_for_seconds

        threshold = self.threshold

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "open_for_seconds": open_for_seconds,
                "threshold": threshold,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        open_for_seconds = d.pop("open_for_seconds")

        threshold = d.pop("threshold")

        effective_breaker = cls(
            open_for_seconds=open_for_seconds,
            threshold=threshold,
        )

        effective_breaker.additional_properties = d
        return effective_breaker

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
