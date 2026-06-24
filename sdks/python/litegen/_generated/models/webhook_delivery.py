import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="WebhookDelivery")


@_attrs_define
class WebhookDelivery:
    """One row in the `webhook_deliveries` table.

    Attributes:
        attempt_number (int):
        created_at (datetime.datetime):
        generation_id (str):
        id (str):
        key_id (str):
        payload_json (str):
        success (bool): Whether this attempt was successful (HTTP 2xx).
        url (str):
        error_message (Union[None, Unset, str]):
        response_body (Union[None, Unset, str]):
        status_code (Union[None, Unset, int]):
    """

    attempt_number: int
    created_at: datetime.datetime
    generation_id: str
    id: str
    key_id: str
    payload_json: str
    success: bool
    url: str
    error_message: Union[None, Unset, str] = UNSET
    response_body: Union[None, Unset, str] = UNSET
    status_code: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        attempt_number = self.attempt_number

        created_at = self.created_at.isoformat()

        generation_id = self.generation_id

        id = self.id

        key_id = self.key_id

        payload_json = self.payload_json

        success = self.success

        url = self.url

        error_message: Union[None, Unset, str]
        if isinstance(self.error_message, Unset):
            error_message = UNSET
        else:
            error_message = self.error_message

        response_body: Union[None, Unset, str]
        if isinstance(self.response_body, Unset):
            response_body = UNSET
        else:
            response_body = self.response_body

        status_code: Union[None, Unset, int]
        if isinstance(self.status_code, Unset):
            status_code = UNSET
        else:
            status_code = self.status_code

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempt_number": attempt_number,
                "created_at": created_at,
                "generation_id": generation_id,
                "id": id,
                "key_id": key_id,
                "payload_json": payload_json,
                "success": success,
                "url": url,
            }
        )
        if error_message is not UNSET:
            field_dict["error_message"] = error_message
        if response_body is not UNSET:
            field_dict["response_body"] = response_body
        if status_code is not UNSET:
            field_dict["status_code"] = status_code

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        attempt_number = d.pop("attempt_number")

        created_at = isoparse(d.pop("created_at"))

        generation_id = d.pop("generation_id")

        id = d.pop("id")

        key_id = d.pop("key_id")

        payload_json = d.pop("payload_json")

        success = d.pop("success")

        url = d.pop("url")

        def _parse_error_message(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        error_message = _parse_error_message(d.pop("error_message", UNSET))

        def _parse_response_body(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        response_body = _parse_response_body(d.pop("response_body", UNSET))

        def _parse_status_code(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        status_code = _parse_status_code(d.pop("status_code", UNSET))

        webhook_delivery = cls(
            attempt_number=attempt_number,
            created_at=created_at,
            generation_id=generation_id,
            id=id,
            key_id=key_id,
            payload_json=payload_json,
            success=success,
            url=url,
            error_message=error_message,
            response_body=response_body,
            status_code=status_code,
        )

        webhook_delivery.additional_properties = d
        return webhook_delivery

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
