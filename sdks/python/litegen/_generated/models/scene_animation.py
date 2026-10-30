from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SceneAnimation")


@_attrs_define
class SceneAnimation:
    """
    Attributes:
        duration_s (float): Measured from the keys.
        in_place (bool): Measured: the root ends within 1 cm of where it started, horizontally.
        name (str):
        root_travel_m (List[float]): The root's world displacement, first key to last, in metres.
        in_place_by (Union[None, Unset, str]): `vendor` (arrived in place), `litegen` (litegen removed the root's
            horizontal travel), or absent when not in place.
        loop_requested (Union[None, Unset, bool]): Whether the request asked for a loop (a hint; glTF has no loop flag).
    """

    duration_s: float
    in_place: bool
    name: str
    root_travel_m: List[float]
    in_place_by: Union[None, Unset, str] = UNSET
    loop_requested: Union[None, Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        duration_s = self.duration_s

        in_place = self.in_place

        name = self.name

        root_travel_m = self.root_travel_m

        in_place_by: Union[None, Unset, str]
        if isinstance(self.in_place_by, Unset):
            in_place_by = UNSET
        else:
            in_place_by = self.in_place_by

        loop_requested: Union[None, Unset, bool]
        if isinstance(self.loop_requested, Unset):
            loop_requested = UNSET
        else:
            loop_requested = self.loop_requested

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "duration_s": duration_s,
                "in_place": in_place,
                "name": name,
                "root_travel_m": root_travel_m,
            }
        )
        if in_place_by is not UNSET:
            field_dict["in_place_by"] = in_place_by
        if loop_requested is not UNSET:
            field_dict["loop_requested"] = loop_requested

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        duration_s = d.pop("duration_s")

        in_place = d.pop("in_place")

        name = d.pop("name")

        root_travel_m = cast(List[float], d.pop("root_travel_m"))

        def _parse_in_place_by(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        in_place_by = _parse_in_place_by(d.pop("in_place_by", UNSET))

        def _parse_loop_requested(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        loop_requested = _parse_loop_requested(d.pop("loop_requested", UNSET))

        scene_animation = cls(
            duration_s=duration_s,
            in_place=in_place,
            name=name,
            root_travel_m=root_travel_m,
            in_place_by=in_place_by,
            loop_requested=loop_requested,
        )

        scene_animation.additional_properties = d
        return scene_animation

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
