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

T = TypeVar("T", bound="User")


@_attrs_define
class User:
    """
    Attributes:
        created_at (datetime.datetime):
        email (str):
        id (str):
        is_active (bool):
        role (Role):
        updated_at (datetime.datetime):
        last_login_at (Union[None, Unset, datetime.datetime]):
        oauth_github_id (Union[None, Unset, str]):
        oauth_google_id (Union[None, Unset, str]):
    """

    created_at: datetime.datetime
    email: str
    id: str
    is_active: bool
    role: Role
    updated_at: datetime.datetime
    last_login_at: Union[None, Unset, datetime.datetime] = UNSET
    oauth_github_id: Union[None, Unset, str] = UNSET
    oauth_google_id: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        email = self.email

        id = self.id

        is_active = self.is_active

        role = self.role.value

        updated_at = self.updated_at.isoformat()

        last_login_at: Union[None, Unset, str]
        if isinstance(self.last_login_at, Unset):
            last_login_at = UNSET
        elif isinstance(self.last_login_at, datetime.datetime):
            last_login_at = self.last_login_at.isoformat()
        else:
            last_login_at = self.last_login_at

        oauth_github_id: Union[None, Unset, str]
        if isinstance(self.oauth_github_id, Unset):
            oauth_github_id = UNSET
        else:
            oauth_github_id = self.oauth_github_id

        oauth_google_id: Union[None, Unset, str]
        if isinstance(self.oauth_google_id, Unset):
            oauth_google_id = UNSET
        else:
            oauth_google_id = self.oauth_google_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "email": email,
                "id": id,
                "is_active": is_active,
                "role": role,
                "updated_at": updated_at,
            }
        )
        if last_login_at is not UNSET:
            field_dict["last_login_at"] = last_login_at
        if oauth_github_id is not UNSET:
            field_dict["oauth_github_id"] = oauth_github_id
        if oauth_google_id is not UNSET:
            field_dict["oauth_google_id"] = oauth_google_id

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        email = d.pop("email")

        id = d.pop("id")

        is_active = d.pop("is_active")

        role = Role(d.pop("role"))

        updated_at = isoparse(d.pop("updated_at"))

        def _parse_last_login_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_login_at_type_0 = isoparse(data)

                return last_login_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        last_login_at = _parse_last_login_at(d.pop("last_login_at", UNSET))

        def _parse_oauth_github_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        oauth_github_id = _parse_oauth_github_id(d.pop("oauth_github_id", UNSET))

        def _parse_oauth_google_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        oauth_google_id = _parse_oauth_google_id(d.pop("oauth_google_id", UNSET))

        user = cls(
            created_at=created_at,
            email=email,
            id=id,
            is_active=is_active,
            role=role,
            updated_at=updated_at,
            last_login_at=last_login_at,
            oauth_github_id=oauth_github_id,
            oauth_google_id=oauth_google_id,
        )

        user.additional_properties = d
        return user

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
