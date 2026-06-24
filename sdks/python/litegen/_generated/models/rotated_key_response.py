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

T = TypeVar("T", bound="RotatedKeyResponse")


@_attrs_define
class RotatedKeyResponse:
    """Response for `POST /v1/keys/{id}/rotate`. Carries the freshly-minted secret,
    which is returned exactly once (it is never persisted or shown again).

        Attributes:
            id (UUID):
            key (str): The new secret, e.g. "sk_live_…". Shown only at rotation time.
            name (str):
            prefix (str):
            public_id (str): Public key id, e.g. "pk_live_…".
            scopes (str): CSV of scopes carried over from the rotated key.
            expires_at (Union[None, Unset, datetime.datetime]):
            rpm_limit (Union[None, Unset, int]):
            token_quota (Union[None, Unset, float]):
            webhook_url (Union[None, Unset, str]):
    """

    id: UUID
    key: str
    name: str
    prefix: str
    public_id: str
    scopes: str
    expires_at: Union[None, Unset, datetime.datetime] = UNSET
    rpm_limit: Union[None, Unset, int] = UNSET
    token_quota: Union[None, Unset, float] = UNSET
    webhook_url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        id = str(self.id)

        key = self.key

        name = self.name

        prefix = self.prefix

        public_id = self.public_id

        scopes = self.scopes

        expires_at: Union[None, Unset, str]
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

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
                "id": id,
                "key": key,
                "name": name,
                "prefix": prefix,
                "public_id": public_id,
                "scopes": scopes,
            }
        )
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at
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
        id = UUID(d.pop("id"))

        key = d.pop("key")

        name = d.pop("name")

        prefix = d.pop("prefix")

        public_id = d.pop("public_id")

        scopes = d.pop("scopes")

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

        rotated_key_response = cls(
            id=id,
            key=key,
            name=name,
            prefix=prefix,
            public_id=public_id,
            scopes=scopes,
            expires_at=expires_at,
            rpm_limit=rpm_limit,
            token_quota=token_quota,
            webhook_url=webhook_url,
        )

        rotated_key_response.additional_properties = d
        return rotated_key_response

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
