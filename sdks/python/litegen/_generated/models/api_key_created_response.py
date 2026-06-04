import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApiKeyCreatedResponse")


@_attrs_define
class ApiKeyCreatedResponse:
    """Response for `POST /v1/keys`.

    Attributes:
        created_at (datetime.datetime):
        id (UUID):
        key (str):
        name (str):
        prefix (str):
        public_id (str):
        scopes (str):
        rpm_limit (Union[None, Unset, int]):
        token_quota (Union[None, Unset, float]):
    """

    created_at: datetime.datetime
    id: UUID
    key: str
    name: str
    prefix: str
    public_id: str
    scopes: str
    rpm_limit: Union[None, Unset, int] = UNSET
    token_quota: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        id = str(self.id)

        key = self.key

        name = self.name

        prefix = self.prefix

        public_id = self.public_id

        scopes = self.scopes

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

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "id": id,
                "key": key,
                "name": name,
                "prefix": prefix,
                "public_id": public_id,
                "scopes": scopes,
            }
        )
        if rpm_limit is not UNSET:
            field_dict["rpm_limit"] = rpm_limit
        if token_quota is not UNSET:
            field_dict["token_quota"] = token_quota

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        id = UUID(d.pop("id"))

        key = d.pop("key")

        name = d.pop("name")

        prefix = d.pop("prefix")

        public_id = d.pop("public_id")

        scopes = d.pop("scopes")

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

        api_key_created_response = cls(
            created_at=created_at,
            id=id,
            key=key,
            name=name,
            prefix=prefix,
            public_id=public_id,
            scopes=scopes,
            rpm_limit=rpm_limit,
            token_quota=token_quota,
        )

        api_key_created_response.additional_properties = d
        return api_key_created_response

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
