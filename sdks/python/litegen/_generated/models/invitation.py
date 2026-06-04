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

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.role import Role
from ..types import UNSET, Unset

T = TypeVar("T", bound="Invitation")


@_attrs_define
class Invitation:
    """
    Attributes:
        created_at (datetime.datetime):
        email (str):
        expires_at (datetime.datetime):
        id (str):
        org_id (str): Organization the invitee will join on accept. Defaults to the
            single-tenant default org for legacy/global invites.
        role (Role):
        token (str):
        invited_by (Union[None, Unset, str]):
        used_at (Union[None, Unset, datetime.datetime]):
    """

    created_at: datetime.datetime
    email: str
    expires_at: datetime.datetime
    id: str
    org_id: str
    role: Role
    token: str
    invited_by: Union[None, Unset, str] = UNSET
    used_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        email = self.email

        expires_at = self.expires_at.isoformat()

        id = self.id

        org_id = self.org_id

        role = self.role.value

        token = self.token

        invited_by: Union[None, Unset, str]
        if isinstance(self.invited_by, Unset):
            invited_by = UNSET
        else:
            invited_by = self.invited_by

        used_at: Union[None, Unset, str]
        if isinstance(self.used_at, Unset):
            used_at = UNSET
        elif isinstance(self.used_at, datetime.datetime):
            used_at = self.used_at.isoformat()
        else:
            used_at = self.used_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "email": email,
                "expires_at": expires_at,
                "id": id,
                "org_id": org_id,
                "role": role,
                "token": token,
            }
        )
        if invited_by is not UNSET:
            field_dict["invited_by"] = invited_by
        if used_at is not UNSET:
            field_dict["used_at"] = used_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        email = d.pop("email")

        expires_at = isoparse(d.pop("expires_at"))

        id = d.pop("id")

        org_id = d.pop("org_id")

        role = Role(d.pop("role"))

        token = d.pop("token")

        def _parse_invited_by(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        invited_by = _parse_invited_by(d.pop("invited_by", UNSET))

        def _parse_used_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                used_at_type_0 = isoparse(data)

                return used_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        used_at = _parse_used_at(d.pop("used_at", UNSET))

        invitation = cls(
            created_at=created_at,
            email=email,
            expires_at=expires_at,
            id=id,
            org_id=org_id,
            role=role,
            token=token,
            invited_by=invited_by,
            used_at=used_at,
        )

        invitation.additional_properties = d
        return invitation

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
