from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.route_deployment import RouteDeployment


T = TypeVar("T", bound="RouteSpec")


@_attrs_define
class RouteSpec:
    """A model route: which providers serve models matching `model`, and how.

    Attributes:
        deployments (List['RouteDeployment']):
        model (str): Exact model id, `prefix/*` or `*`.
        cache_enabled (Union[None, Unset, bool]):
        cache_ttl_seconds (Union[None, Unset, int]):
        strategy (Union[None, Unset, str]): `fallback` (default), `weighted_round_robin`, `lowest_cost` or
            `lowest_latency`.
    """

    deployments: List["RouteDeployment"]
    model: str
    cache_enabled: Union[None, Unset, bool] = UNSET
    cache_ttl_seconds: Union[None, Unset, int] = UNSET
    strategy: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        deployments = []
        for deployments_item_data in self.deployments:
            deployments_item = deployments_item_data.to_dict()
            deployments.append(deployments_item)

        model = self.model

        cache_enabled: Union[None, Unset, bool]
        if isinstance(self.cache_enabled, Unset):
            cache_enabled = UNSET
        else:
            cache_enabled = self.cache_enabled

        cache_ttl_seconds: Union[None, Unset, int]
        if isinstance(self.cache_ttl_seconds, Unset):
            cache_ttl_seconds = UNSET
        else:
            cache_ttl_seconds = self.cache_ttl_seconds

        strategy: Union[None, Unset, str]
        if isinstance(self.strategy, Unset):
            strategy = UNSET
        else:
            strategy = self.strategy

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployments": deployments,
                "model": model,
            }
        )
        if cache_enabled is not UNSET:
            field_dict["cache_enabled"] = cache_enabled
        if cache_ttl_seconds is not UNSET:
            field_dict["cache_ttl_seconds"] = cache_ttl_seconds
        if strategy is not UNSET:
            field_dict["strategy"] = strategy

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.route_deployment import RouteDeployment

        d = src_dict.copy()
        deployments = []
        _deployments = d.pop("deployments")
        for deployments_item_data in _deployments:
            deployments_item = RouteDeployment.from_dict(deployments_item_data)

            deployments.append(deployments_item)

        model = d.pop("model")

        def _parse_cache_enabled(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        cache_enabled = _parse_cache_enabled(d.pop("cache_enabled", UNSET))

        def _parse_cache_ttl_seconds(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        cache_ttl_seconds = _parse_cache_ttl_seconds(d.pop("cache_ttl_seconds", UNSET))

        def _parse_strategy(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        strategy = _parse_strategy(d.pop("strategy", UNSET))

        route_spec = cls(
            deployments=deployments,
            model=model,
            cache_enabled=cache_enabled,
            cache_ttl_seconds=cache_ttl_seconds,
            strategy=strategy,
        )

        route_spec.additional_properties = d
        return route_spec

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
