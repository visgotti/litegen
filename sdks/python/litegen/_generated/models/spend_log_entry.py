import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="SpendLogEntry")


@_attrs_define
class SpendLogEntry:
    """One request in the spend log, with its attribution.

    Attributes:
        cost_usd (float):
        created_at (datetime.datetime):
        id (str):
        latency_ms (int):
        media_type (str):
        model (str):
        provider (str):
        status (str):
        tags (List[str]):
        app_id (Union[None, Unset, str]):
        end_user (Union[None, Unset, str]):
        key_id (Union[None, Unset, str]):
        org_id (Union[None, Unset, str]):
        user_id (Union[None, Unset, str]):
    """

    cost_usd: float
    created_at: datetime.datetime
    id: str
    latency_ms: int
    media_type: str
    model: str
    provider: str
    status: str
    tags: List[str]
    app_id: Union[None, Unset, str] = UNSET
    end_user: Union[None, Unset, str] = UNSET
    key_id: Union[None, Unset, str] = UNSET
    org_id: Union[None, Unset, str] = UNSET
    user_id: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cost_usd = self.cost_usd

        created_at = self.created_at.isoformat()

        id = self.id

        latency_ms = self.latency_ms

        media_type = self.media_type

        model = self.model

        provider = self.provider

        status = self.status

        tags = self.tags

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        end_user: Union[None, Unset, str]
        if isinstance(self.end_user, Unset):
            end_user = UNSET
        else:
            end_user = self.end_user

        key_id: Union[None, Unset, str]
        if isinstance(self.key_id, Unset):
            key_id = UNSET
        else:
            key_id = self.key_id

        org_id: Union[None, Unset, str]
        if isinstance(self.org_id, Unset):
            org_id = UNSET
        else:
            org_id = self.org_id

        user_id: Union[None, Unset, str]
        if isinstance(self.user_id, Unset):
            user_id = UNSET
        else:
            user_id = self.user_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cost_usd": cost_usd,
                "created_at": created_at,
                "id": id,
                "latency_ms": latency_ms,
                "media_type": media_type,
                "model": model,
                "provider": provider,
                "status": status,
                "tags": tags,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id
        if end_user is not UNSET:
            field_dict["end_user"] = end_user
        if key_id is not UNSET:
            field_dict["key_id"] = key_id
        if org_id is not UNSET:
            field_dict["org_id"] = org_id
        if user_id is not UNSET:
            field_dict["user_id"] = user_id

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        cost_usd = d.pop("cost_usd")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        latency_ms = d.pop("latency_ms")

        media_type = d.pop("media_type")

        model = d.pop("model")

        provider = d.pop("provider")

        status = d.pop("status")

        tags = cast(List[str], d.pop("tags"))

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        def _parse_end_user(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        end_user = _parse_end_user(d.pop("end_user", UNSET))

        def _parse_key_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        key_id = _parse_key_id(d.pop("key_id", UNSET))

        def _parse_org_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_id = _parse_org_id(d.pop("org_id", UNSET))

        def _parse_user_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        user_id = _parse_user_id(d.pop("user_id", UNSET))

        spend_log_entry = cls(
            cost_usd=cost_usd,
            created_at=created_at,
            id=id,
            latency_ms=latency_ms,
            media_type=media_type,
            model=model,
            provider=provider,
            status=status,
            tags=tags,
            app_id=app_id,
            end_user=end_user,
            key_id=key_id,
            org_id=org_id,
            user_id=user_id,
        )

        spend_log_entry.additional_properties = d
        return spend_log_entry

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
