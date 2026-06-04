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

from ..types import UNSET, Unset

T = TypeVar("T", bound="PasswordReset")


@_attrs_define
class PasswordReset:
    """
    Attributes:
        created_at (datetime.datetime):
        expires_at (datetime.datetime):
        token (str):
        user_id (str):
        used_at (Union[None, Unset, datetime.datetime]):
    """

    created_at: datetime.datetime
    expires_at: datetime.datetime
    token: str
    user_id: str
    used_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        expires_at = self.expires_at.isoformat()

        token = self.token

        user_id = self.user_id

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
                "expires_at": expires_at,
                "token": token,
                "user_id": user_id,
            }
        )
        if used_at is not UNSET:
            field_dict["used_at"] = used_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        expires_at = isoparse(d.pop("expires_at"))

        token = d.pop("token")

        user_id = d.pop("user_id")

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

        password_reset = cls(
            created_at=created_at,
            expires_at=expires_at,
            token=token,
            user_id=user_id,
            used_at=used_at,
        )

        password_reset.additional_properties = d
        return password_reset

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
