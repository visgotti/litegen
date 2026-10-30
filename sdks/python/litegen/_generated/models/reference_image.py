from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.ref_image_kind import RefImageKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="ReferenceImage")


@_attrs_define
class ReferenceImage:
    """
    Attributes:
        type (RefImageKind):
        value (str):
        role (Union[None, Unset, str]):
        time_s (Union[None, Unset, float]): Seconds from the start of the clip, on a `keyframe` reference (3D doc
            §5): the video shows this image at that time.
    """

    type: RefImageKind
    value: str
    role: Union[None, Unset, str] = UNSET
    time_s: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        type = self.type.value

        value = self.value

        role: Union[None, Unset, str]
        if isinstance(self.role, Unset):
            role = UNSET
        else:
            role = self.role

        time_s: Union[None, Unset, float]
        if isinstance(self.time_s, Unset):
            time_s = UNSET
        else:
            time_s = self.time_s

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "type": type,
                "value": value,
            }
        )
        if role is not UNSET:
            field_dict["role"] = role
        if time_s is not UNSET:
            field_dict["time_s"] = time_s

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        type = RefImageKind(d.pop("type"))

        value = d.pop("value")

        def _parse_role(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        role = _parse_role(d.pop("role", UNSET))

        def _parse_time_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        time_s = _parse_time_s(d.pop("time_s", UNSET))

        reference_image = cls(
            type=type,
            value=value,
            role=role,
            time_s=time_s,
        )

        reference_image.additional_properties = d
        return reference_image

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
