from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PromptSpec")


@_attrs_define
class PromptSpec:
    """
    Attributes:
        max_length (Union[None, Unset, int]):
        min_length (Union[None, Unset, int]):
        required (Union[Unset, bool]):
    """

    max_length: Union[None, Unset, int] = UNSET
    min_length: Union[None, Unset, int] = UNSET
    required: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_length: Union[None, Unset, int]
        if isinstance(self.max_length, Unset):
            max_length = UNSET
        else:
            max_length = self.max_length

        min_length: Union[None, Unset, int]
        if isinstance(self.min_length, Unset):
            min_length = UNSET
        else:
            min_length = self.min_length

        required = self.required

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if max_length is not UNSET:
            field_dict["max_length"] = max_length
        if min_length is not UNSET:
            field_dict["min_length"] = min_length
        if required is not UNSET:
            field_dict["required"] = required

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_max_length(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_length = _parse_max_length(d.pop("max_length", UNSET))

        def _parse_min_length(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        min_length = _parse_min_length(d.pop("min_length", UNSET))

        required = d.pop("required", UNSET)

        prompt_spec = cls(
            max_length=max_length,
            min_length=min_length,
            required=required,
        )

        prompt_spec.additional_properties = d
        return prompt_spec

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
