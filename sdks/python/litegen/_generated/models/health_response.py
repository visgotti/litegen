from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.cache_status import CacheStatus
    from ..models.provider_health import ProviderHealth


T = TypeVar("T", bound="HealthResponse")


@_attrs_define
class HealthResponse:
    """Response for `GET /health`.

    Attributes:
        cache (CacheStatus): Cache state included in the health response.
        providers (List['ProviderHealth']):
        status (str):
    """

    cache: "CacheStatus"
    providers: List["ProviderHealth"]
    status: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cache = self.cache.to_dict()

        providers = []
        for providers_item_data in self.providers:
            providers_item = providers_item_data.to_dict()
            providers.append(providers_item)

        status = self.status

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cache": cache,
                "providers": providers,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.cache_status import CacheStatus
        from ..models.provider_health import ProviderHealth

        d = src_dict.copy()
        cache = CacheStatus.from_dict(d.pop("cache"))

        providers = []
        _providers = d.pop("providers")
        for providers_item_data in _providers:
            providers_item = ProviderHealth.from_dict(providers_item_data)

            providers.append(providers_item)

        status = d.pop("status")

        health_response = cls(
            cache=cache,
            providers=providers,
            status=status,
        )

        health_response.additional_properties = d
        return health_response

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
