import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.webhook_delivery_status import WebhookDeliveryStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="WebhookDeliveryState")


@_attrs_define
class WebhookDeliveryState:
    """A key's webhook delivery: one terminal webhook of a job, sent until an
    attempt is answered 2xx or its retries run out (docs/management-api.md,
    "Key webhooks").

        Attributes:
            attempts (int): The attempts sent in the current series; a redeliver starts a new
                series at 0.
            created_at (datetime.datetime):
            delivery_id (str): `whd-` and a UUID: the `X-Litegen-Delivery-Id` of every attempt.
            generation_id (str): The job whose webhook this is.
            next_attempt_at (datetime.datetime): When the next attempt is due, while `pending`.
            status (WebhookDeliveryStatus): Where a webhook delivery stands.
            finished_at (Union[None, Unset, datetime.datetime]): When it ended (`delivered` or `failed`).
            last_error (Union[None, Unset, str]): Why the latest attempt failed, or why the delivery can't be sent.
            last_status_code (Union[None, Unset, int]): The latest attempt's HTTP status, when it got an answer.
            redelivered_at (Union[None, Unset, datetime.datetime]): When it was last redelivered.
    """

    attempts: int
    created_at: datetime.datetime
    delivery_id: str
    generation_id: str
    next_attempt_at: datetime.datetime
    status: WebhookDeliveryStatus
    finished_at: Union[None, Unset, datetime.datetime] = UNSET
    last_error: Union[None, Unset, str] = UNSET
    last_status_code: Union[None, Unset, int] = UNSET
    redelivered_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        attempts = self.attempts

        created_at = self.created_at.isoformat()

        delivery_id = self.delivery_id

        generation_id = self.generation_id

        next_attempt_at = self.next_attempt_at.isoformat()

        status = self.status.value

        finished_at: Union[None, Unset, str]
        if isinstance(self.finished_at, Unset):
            finished_at = UNSET
        elif isinstance(self.finished_at, datetime.datetime):
            finished_at = self.finished_at.isoformat()
        else:
            finished_at = self.finished_at

        last_error: Union[None, Unset, str]
        if isinstance(self.last_error, Unset):
            last_error = UNSET
        else:
            last_error = self.last_error

        last_status_code: Union[None, Unset, int]
        if isinstance(self.last_status_code, Unset):
            last_status_code = UNSET
        else:
            last_status_code = self.last_status_code

        redelivered_at: Union[None, Unset, str]
        if isinstance(self.redelivered_at, Unset):
            redelivered_at = UNSET
        elif isinstance(self.redelivered_at, datetime.datetime):
            redelivered_at = self.redelivered_at.isoformat()
        else:
            redelivered_at = self.redelivered_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "attempts": attempts,
                "created_at": created_at,
                "delivery_id": delivery_id,
                "generation_id": generation_id,
                "next_attempt_at": next_attempt_at,
                "status": status,
            }
        )
        if finished_at is not UNSET:
            field_dict["finished_at"] = finished_at
        if last_error is not UNSET:
            field_dict["last_error"] = last_error
        if last_status_code is not UNSET:
            field_dict["last_status_code"] = last_status_code
        if redelivered_at is not UNSET:
            field_dict["redelivered_at"] = redelivered_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        attempts = d.pop("attempts")

        created_at = isoparse(d.pop("created_at"))

        delivery_id = d.pop("delivery_id")

        generation_id = d.pop("generation_id")

        next_attempt_at = isoparse(d.pop("next_attempt_at"))

        status = WebhookDeliveryStatus(d.pop("status"))

        def _parse_finished_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                finished_at_type_0 = isoparse(data)

                return finished_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        finished_at = _parse_finished_at(d.pop("finished_at", UNSET))

        def _parse_last_error(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        last_error = _parse_last_error(d.pop("last_error", UNSET))

        def _parse_last_status_code(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        last_status_code = _parse_last_status_code(d.pop("last_status_code", UNSET))

        def _parse_redelivered_at(
            data: object,
        ) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                redelivered_at_type_0 = isoparse(data)

                return redelivered_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        redelivered_at = _parse_redelivered_at(d.pop("redelivered_at", UNSET))

        webhook_delivery_state = cls(
            attempts=attempts,
            created_at=created_at,
            delivery_id=delivery_id,
            generation_id=generation_id,
            next_attempt_at=next_attempt_at,
            status=status,
            finished_at=finished_at,
            last_error=last_error,
            last_status_code=last_status_code,
            redelivered_at=redelivered_at,
        )

        webhook_delivery_state.additional_properties = d
        return webhook_delivery_state

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
