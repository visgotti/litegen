from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CacheConfig")


@_attrs_define
class CacheConfig:
    """
    Attributes:
        enabled (Union[Unset, bool]): Enable caching for this model.
        max_items (Union[Unset, int]): Max cached items.
        ttl_seconds (Union[Unset, int]): Cache TTL in seconds.
    """

    enabled: Union[Unset, bool] = UNSET
    max_items: Union[Unset, int] = UNSET
    ttl_seconds: Union[Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        enabled = self.enabled

        max_items = self.max_items

        ttl_seconds = self.ttl_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if max_items is not UNSET:
            field_dict["max_items"] = max_items
        if ttl_seconds is not UNSET:
            field_dict["ttl_seconds"] = ttl_seconds

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        enabled = d.pop("enabled", UNSET)

        max_items = d.pop("max_items", UNSET)

        ttl_seconds = d.pop("ttl_seconds", UNSET)

        cache_config = cls(
            enabled=enabled,
            max_items=max_items,
            ttl_seconds=ttl_seconds,
        )

        cache_config.additional_properties = d
        return cache_config

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
