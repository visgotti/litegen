from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecString")


@_attrs_define
class ParamSpecString:
    """
    Attributes:
        default (Union[None, Unset, str]):
        description (Union[None, Unset, str]):
        enum_values (Union[Unset, List[str]]):
        label (Union[None, Unset, str]):
        max_length (Union[None, Unset, int]):
        pattern (Union[None, Unset, str]):
    """

    default: Union[None, Unset, str] = UNSET
    description: Union[None, Unset, str] = UNSET
    enum_values: Union[Unset, List[str]] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_length: Union[None, Unset, int] = UNSET
    pattern: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
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

        enum_values: Union[Unset, List[str]] = UNSET
        if not isinstance(self.enum_values, Unset):
            enum_values = self.enum_values

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        max_length: Union[None, Unset, int]
        if isinstance(self.max_length, Unset):
            max_length = UNSET
        else:
            max_length = self.max_length

        pattern: Union[None, Unset, str]
        if isinstance(self.pattern, Unset):
            pattern = UNSET
        else:
            pattern = self.pattern

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if default is not UNSET:
            field_dict["default"] = default
        if description is not UNSET:
            field_dict["description"] = description
        if enum_values is not UNSET:
            field_dict["enum_values"] = enum_values
        if label is not UNSET:
            field_dict["label"] = label
        if max_length is not UNSET:
            field_dict["max_length"] = max_length
        if pattern is not UNSET:
            field_dict["pattern"] = pattern

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

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

        enum_values = cast(List[str], d.pop("enum_values", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_max_length(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_length = _parse_max_length(d.pop("max_length", UNSET))

        def _parse_pattern(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        pattern = _parse_pattern(d.pop("pattern", UNSET))

        param_spec_string = cls(
            default=default,
            description=description,
            enum_values=enum_values,
            label=label,
            max_length=max_length,
            pattern=pattern,
        )

        param_spec_string.additional_properties = d
        return param_spec_string

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
