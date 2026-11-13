from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="EffectiveCache")


@_attrs_define
class EffectiveCache:
    """The cache in force (`GET /v1/admin/settings`).

    Attributes:
        enabled (bool):
        max_bytes (int): About how many bytes each replica's cache may hold.
        max_items (int):
        ttl_seconds (int):
    """

    enabled: bool
    max_bytes: int
    max_items: int
    ttl_seconds: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        enabled = self.enabled

        max_bytes = self.max_bytes

        max_items = self.max_items

        ttl_seconds = self.ttl_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "enabled": enabled,
                "max_bytes": max_bytes,
                "max_items": max_items,
                "ttl_seconds": ttl_seconds,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        enabled = d.pop("enabled")

        max_bytes = d.pop("max_bytes")

        max_items = d.pop("max_items")

        ttl_seconds = d.pop("ttl_seconds")

        effective_cache = cls(
            enabled=enabled,
            max_bytes=max_bytes,
            max_items=max_items,
            ttl_seconds=ttl_seconds,
        )

        effective_cache.additional_properties = d
        return effective_cache

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
