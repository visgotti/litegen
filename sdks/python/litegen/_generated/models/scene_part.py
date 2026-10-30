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

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.scene_bounds import SceneBounds


T = TypeVar("T", bound="ScenePart")


@_attrs_define
class ScenePart:
    """One part of a segmented mesh, measured by litegen.

    Attributes:
        name (str):
        node (int): The part's node: its index in the delivered glTF's `nodes`.
        triangle_count (int):
        bounds (Union['SceneBounds', None, Unset]):
    """

    name: str
    node: int
    triangle_count: int
    bounds: Union["SceneBounds", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.scene_bounds import SceneBounds

        name = self.name

        node = self.node

        triangle_count = self.triangle_count

        bounds: Union[Dict[str, Any], None, Unset]
        if isinstance(self.bounds, Unset):
            bounds = UNSET
        elif isinstance(self.bounds, SceneBounds):
            bounds = self.bounds.to_dict()
        else:
            bounds = self.bounds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
                "node": node,
                "triangle_count": triangle_count,
            }
        )
        if bounds is not UNSET:
            field_dict["bounds"] = bounds

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.scene_bounds import SceneBounds

        d = src_dict.copy()
        name = d.pop("name")

        node = d.pop("node")

        triangle_count = d.pop("triangle_count")

        def _parse_bounds(data: object) -> Union["SceneBounds", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                bounds_type_1 = SceneBounds.from_dict(data)

                return bounds_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SceneBounds", None, Unset], data)

        bounds = _parse_bounds(d.pop("bounds", UNSET))

        scene_part = cls(
            name=name,
            node=node,
            triangle_count=triangle_count,
            bounds=bounds,
        )

        scene_part.additional_properties = d
        return scene_part

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
