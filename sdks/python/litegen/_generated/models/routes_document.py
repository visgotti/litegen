from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.route_spec import RouteSpec


T = TypeVar("T", bound="RoutesDocument")


@_attrs_define
class RoutesDocument:
    """Body and response for `GET`/`PUT /v1/admin/routes`.

    Attributes:
        routes (List['RouteSpec']):
        source (Union[None, Unset, str]): `config` while the routes come from litegen.yaml, `runtime` once replaced
            through the API.
    """

    routes: List["RouteSpec"]
    source: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        routes = []
        for routes_item_data in self.routes:
            routes_item = routes_item_data.to_dict()
            routes.append(routes_item)

        source: Union[None, Unset, str]
        if isinstance(self.source, Unset):
            source = UNSET
        else:
            source = self.source

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "routes": routes,
            }
        )
        if source is not UNSET:
            field_dict["source"] = source

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.route_spec import RouteSpec

        d = src_dict.copy()
        routes = []
        _routes = d.pop("routes")
        for routes_item_data in _routes:
            routes_item = RouteSpec.from_dict(routes_item_data)

            routes.append(routes_item)

        def _parse_source(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        source = _parse_source(d.pop("source", UNSET))

        routes_document = cls(
            routes=routes,
            source=source,
        )

        routes_document.additional_properties = d
        return routes_document

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
