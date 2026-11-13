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
    from ..models.vendor_error import VendorError


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
        generation_id (Union[None, Unset, str]): On an image `vendor_timeout` whose job the vendor is still running:
            the generation litegen delivers it to when it finishes
            (`GET /v1/generations/{id}`, the key's webhook; docs/async-images.md).
        model (Union[None, Unset, str]): The model the rejected request named. Validation errors only.
        param (Union[None, Unset, str]): The request field that failed validation — `output_formats`, `prompt`,
            `reference_images`, `extra.<key>` — so a caller can tell which one to fix
            without parsing `message`. Validation errors only.
        provider_error (Union['VendorError', None, Unset]):
        replaced_by (Union[None, Unset, str]): On `model_retired` (410): the model that replaces the retired one,
            when the catalog names one.
        retryable (Union[None, Unset, bool]): Whether the same request may work later: a vendor's 5xx, 429 or
            timeout, but not its refusal.
        status (Union[None, Unset, str]): `processing`, with `generation_id`: the job runs on.
        vendor_job_id (Union[None, Unset, str]): The vendor's own job id, on `vendor_timeout` (the job the vendor had
            accepted when litegen stopped waiting, if it had; not re-sent) and on
            `generation_record_failed` (a job the vendor took that litegen could
            not record).
    """

    message: str
    type: str
    code: Union[None, Unset, str] = UNSET
    generation_id: Union[None, Unset, str] = UNSET
    model: Union[None, Unset, str] = UNSET
    param: Union[None, Unset, str] = UNSET
    provider_error: Union["VendorError", None, Unset] = UNSET
    replaced_by: Union[None, Unset, str] = UNSET
    retryable: Union[None, Unset, bool] = UNSET
    status: Union[None, Unset, str] = UNSET
    vendor_job_id: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.vendor_error import VendorError

        message = self.message

        type = self.type

        code: Union[None, Unset, str]
        if isinstance(self.code, Unset):
            code = UNSET
        else:
            code = self.code

        generation_id: Union[None, Unset, str]
        if isinstance(self.generation_id, Unset):
            generation_id = UNSET
        else:
            generation_id = self.generation_id

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

        provider_error: Union[Dict[str, Any], None, Unset]
        if isinstance(self.provider_error, Unset):
            provider_error = UNSET
        elif isinstance(self.provider_error, VendorError):
            provider_error = self.provider_error.to_dict()
        else:
            provider_error = self.provider_error

        replaced_by: Union[None, Unset, str]
        if isinstance(self.replaced_by, Unset):
            replaced_by = UNSET
        else:
            replaced_by = self.replaced_by

        retryable: Union[None, Unset, bool]
        if isinstance(self.retryable, Unset):
            retryable = UNSET
        else:
            retryable = self.retryable

        status: Union[None, Unset, str]
        if isinstance(self.status, Unset):
            status = UNSET
        else:
            status = self.status

        vendor_job_id: Union[None, Unset, str]
        if isinstance(self.vendor_job_id, Unset):
            vendor_job_id = UNSET
        else:
            vendor_job_id = self.vendor_job_id

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
        if generation_id is not UNSET:
            field_dict["generation_id"] = generation_id
        if model is not UNSET:
            field_dict["model"] = model
        if param is not UNSET:
            field_dict["param"] = param
        if provider_error is not UNSET:
            field_dict["provider_error"] = provider_error
        if replaced_by is not UNSET:
            field_dict["replaced_by"] = replaced_by
        if retryable is not UNSET:
            field_dict["retryable"] = retryable
        if status is not UNSET:
            field_dict["status"] = status
        if vendor_job_id is not UNSET:
            field_dict["vendor_job_id"] = vendor_job_id

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.vendor_error import VendorError

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

        def _parse_generation_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        generation_id = _parse_generation_id(d.pop("generation_id", UNSET))

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

        def _parse_provider_error(data: object) -> Union["VendorError", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                provider_error_type_1 = VendorError.from_dict(data)

                return provider_error_type_1
            except:  # noqa: E722
                pass
            return cast(Union["VendorError", None, Unset], data)

        provider_error = _parse_provider_error(d.pop("provider_error", UNSET))

        def _parse_replaced_by(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        replaced_by = _parse_replaced_by(d.pop("replaced_by", UNSET))

        def _parse_retryable(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        retryable = _parse_retryable(d.pop("retryable", UNSET))

        def _parse_status(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        status = _parse_status(d.pop("status", UNSET))

        def _parse_vendor_job_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        vendor_job_id = _parse_vendor_job_id(d.pop("vendor_job_id", UNSET))

        error_detail = cls(
            message=message,
            type=type,
            code=code,
            generation_id=generation_id,
            model=model,
            param=param,
            provider_error=provider_error,
            replaced_by=replaced_by,
            retryable=retryable,
            status=status,
            vendor_job_id=vendor_job_id,
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
