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

T = TypeVar("T", bound="ApiKey")


@_attrs_define
class ApiKey:
    """API key for authenticating with the LiteGen proxy.

    Attributes:
        created_at (datetime.datetime):
        id (UUID):
        is_active (bool):
        key_hash (str):
        key_prefix (str):
        name (str):
        expires_at (Union[None, Unset, datetime.datetime]):
    """

    created_at: datetime.datetime
    id: UUID
    is_active: bool
    key_hash: str
    key_prefix: str
    name: str
    expires_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        id = str(self.id)

        is_active = self.is_active

        key_hash = self.key_hash

        key_prefix = self.key_prefix

        name = self.name

        expires_at: Union[None, Unset, str]
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "id": id,
                "is_active": is_active,
                "key_hash": key_hash,
                "key_prefix": key_prefix,
                "name": name,
            }
        )
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        id = UUID(d.pop("id"))

        is_active = d.pop("is_active")

        key_hash = d.pop("key_hash")

        key_prefix = d.pop("key_prefix")

        name = d.pop("name")

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

        api_key = cls(
            created_at=created_at,
            id=id,
            is_active=is_active,
            key_hash=key_hash,
            key_prefix=key_prefix,
            name=name,
            expires_at=expires_at,
        )

        api_key.additional_properties = d
        return api_key

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
