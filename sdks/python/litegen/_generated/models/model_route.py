from typing import (
    TYPE_CHECKING,
    Any,
    Dict,
    List,
    Type,
    TypeVar,
    Union,
    cast,
)

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.routing_strategy import RoutingStrategy
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.cache_config import CacheConfig
    from ..models.deployment import Deployment


T = TypeVar("T", bound="ModelRoute")


@_attrs_define
class ModelRoute:
    """Routing configuration for a model, with fallbacks and weights.

    Attributes:
        deployments (List['Deployment']): Ordered list of provider deployments to try.
        model (str): The model ID pattern (e.g. "dall-e-3", "openai/*").
        cache (Union['CacheConfig', None, Unset]):
        strategy (Union[Unset, RoutingStrategy]):
    """

    deployments: List["Deployment"]
    model: str
    cache: Union["CacheConfig", None, Unset] = UNSET
    strategy: Union[Unset, RoutingStrategy] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.cache_config import CacheConfig

        deployments = []
        for deployments_item_data in self.deployments:
            deployments_item = deployments_item_data.to_dict()
            deployments.append(deployments_item)

        model = self.model

        cache: Union[Dict[str, Any], None, Unset]
        if isinstance(self.cache, Unset):
            cache = UNSET
        elif isinstance(self.cache, CacheConfig):
            cache = self.cache.to_dict()
        else:
            cache = self.cache

        strategy: Union[Unset, str] = UNSET
        if not isinstance(self.strategy, Unset):
            strategy = self.strategy.value

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "deployments": deployments,
                "model": model,
            }
        )
        if cache is not UNSET:
            field_dict["cache"] = cache
        if strategy is not UNSET:
            field_dict["strategy"] = strategy

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.cache_config import CacheConfig
        from ..models.deployment import Deployment

        d = src_dict.copy()
        deployments = []
        _deployments = d.pop("deployments")
        for deployments_item_data in _deployments:
            deployments_item = Deployment.from_dict(deployments_item_data)

            deployments.append(deployments_item)

        model = d.pop("model")

        def _parse_cache(data: object) -> Union["CacheConfig", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                cache_type_1 = CacheConfig.from_dict(data)

                return cache_type_1
            except:  # noqa: E722
                pass
            return cast(Union["CacheConfig", None, Unset], data)

        cache = _parse_cache(d.pop("cache", UNSET))

        _strategy = d.pop("strategy", UNSET)
        strategy: Union[Unset, RoutingStrategy]
        if isinstance(_strategy, Unset):
            strategy = UNSET
        else:
            strategy = RoutingStrategy(_strategy)

        model_route = cls(
            deployments=deployments,
            model=model,
            cache=cache,
            strategy=strategy,
        )

        model_route.additional_properties = d
        return model_route

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
