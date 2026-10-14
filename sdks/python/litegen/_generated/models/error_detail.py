from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ErrorDetail")


@_attrs_define
class ErrorDetail:
    """
    Attributes:
        message (str): Human-readable; not for matching on — use `code` and `param`.
        type (str): The error's family, e.g. `validation_error`, `authentication_error`,
            `quota_exceeded`, `provider_not_configured`.
        code (Union[None, Unset, str]): Machine-readable reason within the family, e.g. `param_unsupported`,
            `param_enum_mismatch`, `prompt_required`, `ref_role_required`.
        model (Union[None, Unset, str]): The model the rejected request named. Validation errors only.
        param (Union[None, Unset, str]): The request field that failed validation — `output_formats`, `prompt`,
            `reference_images`, `extra.<key>` — so a caller can tell which one to fix
            without parsing `message`. Validation errors only.
        provider_error (Union[Unset, Any]):
    """

    message: str
    type: str
    code: Union[None, Unset, str] = UNSET
    model: Union[None, Unset, str] = UNSET
    param: Union[None, Unset, str] = UNSET
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

        model: Union[None, Unset, str]
        if isinstance(self.model, Unset):
            model = UNSET
        else:
            model = self.model

        param: Union[None, Unset, str]
        if isinstance(self.param, Unset):
            param = UNSET
        else:
            param = self.param

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
        if model is not UNSET:
            field_dict["model"] = model
        if param is not UNSET:
            field_dict["param"] = param
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

        def _parse_model(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        model = _parse_model(d.pop("model", UNSET))

        def _parse_param(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        param = _parse_param(d.pop("param", UNSET))

        provider_error = d.pop("provider_error", UNSET)

        error_detail = cls(
            message=message,
            type=type,
            code=code,
            model=model,
            param=param,
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
