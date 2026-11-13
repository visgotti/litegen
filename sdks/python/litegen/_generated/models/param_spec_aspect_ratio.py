from typing import (
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

from ..models.frames_set_shape import FramesSetShape
from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecAspectRatio")


@_attrs_define
class ParamSpecAspectRatio:
    """
    Attributes:
        allowed (List[str]):
        default (Union[None, Unset, str]):
        description (Union[None, Unset, str]):
        frames_set_shape (Union[FramesSetShape, None, Unset]):
        label (Union[None, Unset, str]):
    """

    allowed: List[str]
    default: Union[None, Unset, str] = UNSET
    description: Union[None, Unset, str] = UNSET
    frames_set_shape: Union[FramesSetShape, None, Unset] = UNSET
    label: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        allowed = self.allowed

        default: Union[None, Unset, str]
        if isinstance(self.default, Unset):
            default = UNSET
        else:
            default = self.default

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        frames_set_shape: Union[None, Unset, str]
        if isinstance(self.frames_set_shape, Unset):
            frames_set_shape = UNSET
        elif isinstance(self.frames_set_shape, FramesSetShape):
            frames_set_shape = self.frames_set_shape.value
        else:
            frames_set_shape = self.frames_set_shape

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "allowed": allowed,
            }
        )
        if default is not UNSET:
            field_dict["default"] = default
        if description is not UNSET:
            field_dict["description"] = description
        if frames_set_shape is not UNSET:
            field_dict["frames_set_shape"] = frames_set_shape
        if label is not UNSET:
            field_dict["label"] = label

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        allowed = cast(List[str], d.pop("allowed"))

        def _parse_default(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        default = _parse_default(d.pop("default", UNSET))

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_frames_set_shape(data: object) -> Union[FramesSetShape, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                frames_set_shape_type_1 = FramesSetShape(data)

                return frames_set_shape_type_1
            except:  # noqa: E722
                pass
            return cast(Union[FramesSetShape, None, Unset], data)

        frames_set_shape = _parse_frames_set_shape(d.pop("frames_set_shape", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        param_spec_aspect_ratio = cls(
            allowed=allowed,
            default=default,
            description=description,
            frames_set_shape=frames_set_shape,
            label=label,
        )

        param_spec_aspect_ratio.additional_properties = d
        return param_spec_aspect_ratio

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
