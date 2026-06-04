import datetime
from typing import (
    Any,
    Dict,
    List,
    Type,
    TypeVar,
    Union,
    cast,
)
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiKeyInfo")


@_attrs_define
class ApiKeyInfo:
    """Public view of an API key (no hash).

    Attributes:
        created_at (datetime.datetime):
        id (UUID):
        is_active (bool):
        name (str):
        prefix (str):
        scopes (str):
        tokens_used (float):
        app_id (Union[None, Unset, str]):
        expires_at (Union[None, Unset, datetime.datetime]):
        public_id (Union[None, Unset, str]):
        rpm_limit (Union[None, Unset, int]):
        token_quota (Union[None, Unset, float]):
        webhook_url (Union[None, Unset, str]):
    """

    created_at: datetime.datetime
    id: UUID
    is_active: bool
    name: str
    prefix: str
    scopes: str
    tokens_used: float
    app_id: Union[None, Unset, str] = UNSET
    expires_at: Union[None, Unset, datetime.datetime] = UNSET
    public_id: Union[None, Unset, str] = UNSET
    rpm_limit: Union[None, Unset, int] = UNSET
    token_quota: Union[None, Unset, float] = UNSET
    webhook_url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        id = str(self.id)

        is_active = self.is_active

        name = self.name

        prefix = self.prefix

        scopes = self.scopes

        tokens_used = self.tokens_used

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        expires_at: Union[None, Unset, str]
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        public_id: Union[None, Unset, str]
        if isinstance(self.public_id, Unset):
            public_id = UNSET
        else:
            public_id = self.public_id

        rpm_limit: Union[None, Unset, int]
        if isinstance(self.rpm_limit, Unset):
            rpm_limit = UNSET
        else:
            rpm_limit = self.rpm_limit

        token_quota: Union[None, Unset, float]
        if isinstance(self.token_quota, Unset):
            token_quota = UNSET
        else:
            token_quota = self.token_quota

        webhook_url: Union[None, Unset, str]
        if isinstance(self.webhook_url, Unset):
            webhook_url = UNSET
        else:
            webhook_url = self.webhook_url

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "id": id,
                "is_active": is_active,
                "name": name,
                "prefix": prefix,
                "scopes": scopes,
                "tokens_used": tokens_used,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at
        if public_id is not UNSET:
            field_dict["public_id"] = public_id
        if rpm_limit is not UNSET:
            field_dict["rpm_limit"] = rpm_limit
        if token_quota is not UNSET:
            field_dict["token_quota"] = token_quota
        if webhook_url is not UNSET:
            field_dict["webhook_url"] = webhook_url

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        id = UUID(d.pop("id"))

        is_active = d.pop("is_active")

        name = d.pop("name")

        prefix = d.pop("prefix")

        scopes = d.pop("scopes")

        tokens_used = d.pop("tokens_used")

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        def _parse_expires_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                expires_at_type_0 = isoparse(data)

                return expires_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        expires_at = _parse_expires_at(d.pop("expires_at", UNSET))

        def _parse_public_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        public_id = _parse_public_id(d.pop("public_id", UNSET))

        def _parse_rpm_limit(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        rpm_limit = _parse_rpm_limit(d.pop("rpm_limit", UNSET))

        def _parse_token_quota(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        token_quota = _parse_token_quota(d.pop("token_quota", UNSET))

        def _parse_webhook_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        webhook_url = _parse_webhook_url(d.pop("webhook_url", UNSET))

        api_key_info = cls(
            created_at=created_at,
            id=id,
            is_active=is_active,
            name=name,
            prefix=prefix,
            scopes=scopes,
            tokens_used=tokens_used,
            app_id=app_id,
            expires_at=expires_at,
            public_id=public_id,
            rpm_limit=rpm_limit,
            token_quota=token_quota,
            webhook_url=webhook_url,
        )

        api_key_info.additional_properties = d
        return api_key_info

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
