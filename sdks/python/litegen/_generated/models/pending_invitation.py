from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PendingInvitation")


@_attrs_define
class PendingInvitation:
    """An invitation not yet accepted or expired. Its token is shown only when
    it is made: to send a lost link again, revoke the invitation and invite
    anew.

        Attributes:
            created_at (str):
            email (str):
            expires_at (str):
            id (str):
            invited_by (Union[None, str]): The account that sent it; null when the master key did.
            org_id (str):
            role (str):
    """

    created_at: str
    email: str
    expires_at: str
    id: str
    invited_by: Union[None, str]
    org_id: str
    role: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at

        email = self.email

        expires_at = self.expires_at

        id = self.id

        invited_by: Union[None, str]
        invited_by = self.invited_by

        org_id = self.org_id

        role = self.role

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "email": email,
                "expires_at": expires_at,
                "id": id,
                "invited_by": invited_by,
                "org_id": org_id,
                "role": role,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = d.pop("created_at")

        email = d.pop("email")

        expires_at = d.pop("expires_at")

        id = d.pop("id")

        def _parse_invited_by(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        invited_by = _parse_invited_by(d.pop("invited_by"))

        org_id = d.pop("org_id")

        role = d.pop("role")

        pending_invitation = cls(
            created_at=created_at,
            email=email,
            expires_at=expires_at,
            id=id,
            invited_by=invited_by,
            org_id=org_id,
            role=role,
        )

        pending_invitation.additional_properties = d
        return pending_invitation

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
