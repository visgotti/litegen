from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ErrorDetail")


@_attrs_define
class ErrorDetail:
    """
    Attributes:
        message (str):
        type (str):
        code (Union[None, Unset, str]):
        provider_error (Union[Unset, Any]):
    """

    message: str
    type: str
    code: Union[None, Unset, str] = UNSET
    provider_error: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        message = self.message

        type = self.type

        code: Union[None, Unset, str]
        if isinstance(self.code, Unset):
            code = UNSET
        else:
            code = self.code

        provider_error = self.provider_error

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "message": message,
                "type": type,
            }
        )
        if code is not UNSET:
            field_dict["code"] = code
        if provider_error is not UNSET:
            field_dict["provider_error"] = provider_error

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        message = d.pop("message")

        type = d.pop("type")

        def _parse_code(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        code = _parse_code(d.pop("code", UNSET))

        provider_error = d.pop("provider_error", UNSET)

        error_detail = cls(
            message=message,
            type=type,
            code=code,
            provider_error=provider_error,
        )

        error_detail.additional_properties = d
        return error_detail

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
