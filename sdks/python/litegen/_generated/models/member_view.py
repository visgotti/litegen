from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="MemberView")


@_attrs_define
class MemberView:
    """
    Attributes:
        created_at (str):
        email (str):
        org_id (str):
        role (str):
        user_id (str):
    """

    created_at: str
    email: str
    org_id: str
    role: str
    user_id: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at

        email = self.email

        org_id = self.org_id

        role = self.role

        user_id = self.user_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "email": email,
                "org_id": org_id,
                "role": role,
                "user_id": user_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = d.pop("created_at")

        email = d.pop("email")

        org_id = d.pop("org_id")

        role = d.pop("role")

        user_id = d.pop("user_id")

        member_view = cls(
            created_at=created_at,
            email=email,
            org_id=org_id,
            role=role,
            user_id=user_id,
        )

        member_view.additional_properties = d
        return member_view

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
