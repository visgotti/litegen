from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PublicUser")


@_attrs_define
class PublicUser:
    """Public user view returned from user management endpoints.

    Attributes:
        created_at (str):
        email (str):
        id (str):
        is_active (bool):
        role (str):
        last_login_at (Union[None, Unset, str]):
    """

    created_at: str
    email: str
    id: str
    is_active: bool
    role: str
    last_login_at: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at

        email = self.email

        id = self.id

        is_active = self.is_active

        role = self.role

        last_login_at: Union[None, Unset, str]
        if isinstance(self.last_login_at, Unset):
            last_login_at = UNSET
        else:
            last_login_at = self.last_login_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "email": email,
                "id": id,
                "is_active": is_active,
                "role": role,
            }
        )
        if last_login_at is not UNSET:
            field_dict["last_login_at"] = last_login_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = d.pop("created_at")

        email = d.pop("email")

        id = d.pop("id")

        is_active = d.pop("is_active")

        role = d.pop("role")

        def _parse_last_login_at(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        last_login_at = _parse_last_login_at(d.pop("last_login_at", UNSET))

        public_user = cls(
            created_at=created_at,
            email=email,
            id=id,
            is_active=is_active,
            role=role,
            last_login_at=last_login_at,
        )

        public_user.additional_properties = d
        return public_user

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
