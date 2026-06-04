from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="InvitationView")


@_attrs_define
class InvitationView:
    """
    Attributes:
        email (str):
        expires_at (str):
        role (str):
    """

    email: str
    expires_at: str
    role: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        email = self.email

        expires_at = self.expires_at

        role = self.role

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "email": email,
                "expires_at": expires_at,
                "role": role,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        email = d.pop("email")

        expires_at = d.pop("expires_at")

        role = d.pop("role")

        invitation_view = cls(
            email=email,
            expires_at=expires_at,
            role=role,
        )

        invitation_view.additional_properties = d
        return invitation_view

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
