import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="RequestArtifact")


@_attrs_define
class RequestArtifact:
    """Stores the input/output snapshot of a generation request for drill-down.

    Attributes:
        created_at (datetime.datetime):
        media_type (str):
        output_kind (str): "b64" | "url" | "error"
        output_truncated (bool):
        request_id (str):
        app_id (Union[None, Unset, str]):
        error_message (Union[None, Unset, str]):
        negative_prompt (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        output_mime (Union[None, Unset, str]):
        output_value (Union[None, Unset, str]):
        params_json (Union[Unset, Any]):
        prompt (Union[None, Unset, str]):
        refs_meta_json (Union[Unset, Any]):
    """

    created_at: datetime.datetime
    media_type: str
    output_kind: str
    output_truncated: bool
    request_id: str
    app_id: Union[None, Unset, str] = UNSET
    error_message: Union[None, Unset, str] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    org_id: Union[None, Unset, str] = UNSET
    output_mime: Union[None, Unset, str] = UNSET
    output_value: Union[None, Unset, str] = UNSET
    params_json: Union[Unset, Any] = UNSET
    prompt: Union[None, Unset, str] = UNSET
    refs_meta_json: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        media_type = self.media_type

        output_kind = self.output_kind

        output_truncated = self.output_truncated

        request_id = self.request_id

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        error_message: Union[None, Unset, str]
        if isinstance(self.error_message, Unset):
            error_message = UNSET
        else:
            error_message = self.error_message

        negative_prompt: Union[None, Unset, str]
        if isinstance(self.negative_prompt, Unset):
            negative_prompt = UNSET
        else:
            negative_prompt = self.negative_prompt

        org_id: Union[None, Unset, str]
        if isinstance(self.org_id, Unset):
            org_id = UNSET
        else:
            org_id = self.org_id

        output_mime: Union[None, Unset, str]
        if isinstance(self.output_mime, Unset):
            output_mime = UNSET
        else:
            output_mime = self.output_mime

        output_value: Union[None, Unset, str]
        if isinstance(self.output_value, Unset):
            output_value = UNSET
        else:
            output_value = self.output_value

        params_json = self.params_json

        prompt: Union[None, Unset, str]
        if isinstance(self.prompt, Unset):
            prompt = UNSET
        else:
            prompt = self.prompt

        refs_meta_json = self.refs_meta_json

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "media_type": media_type,
                "output_kind": output_kind,
                "output_truncated": output_truncated,
                "request_id": request_id,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id
        if error_message is not UNSET:
            field_dict["error_message"] = error_message
        if negative_prompt is not UNSET:
            field_dict["negative_prompt"] = negative_prompt
        if org_id is not UNSET:
            field_dict["org_id"] = org_id
        if output_mime is not UNSET:
            field_dict["output_mime"] = output_mime
        if output_value is not UNSET:
            field_dict["output_value"] = output_value
        if params_json is not UNSET:
            field_dict["params_json"] = params_json
        if prompt is not UNSET:
            field_dict["prompt"] = prompt
        if refs_meta_json is not UNSET:
            field_dict["refs_meta_json"] = refs_meta_json

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        media_type = d.pop("media_type")

        output_kind = d.pop("output_kind")

        output_truncated = d.pop("output_truncated")

        request_id = d.pop("request_id")

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        def _parse_error_message(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        error_message = _parse_error_message(d.pop("error_message", UNSET))

        def _parse_negative_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        negative_prompt = _parse_negative_prompt(d.pop("negative_prompt", UNSET))

        def _parse_org_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_id = _parse_org_id(d.pop("org_id", UNSET))

        def _parse_output_mime(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        output_mime = _parse_output_mime(d.pop("output_mime", UNSET))

        def _parse_output_value(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        output_value = _parse_output_value(d.pop("output_value", UNSET))

        params_json = d.pop("params_json", UNSET)

        def _parse_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        prompt = _parse_prompt(d.pop("prompt", UNSET))

        refs_meta_json = d.pop("refs_meta_json", UNSET)

        request_artifact = cls(
            created_at=created_at,
            media_type=media_type,
            output_kind=output_kind,
            output_truncated=output_truncated,
            request_id=request_id,
            app_id=app_id,
            error_message=error_message,
            negative_prompt=negative_prompt,
            org_id=org_id,
            output_mime=output_mime,
            output_value=output_value,
            params_json=params_json,
            prompt=prompt,
            refs_meta_json=refs_meta_json,
        )

        request_artifact.additional_properties = d
        return request_artifact

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
