import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="Session")


@_attrs_define
class Session:
    """
    Attributes:
        created_at (datetime.datetime):
        csrf_token (str):
        expires_at (datetime.datetime):
        id (str):
        user_id (str):
        ip (Union[None, Unset, str]):
        user_agent (Union[None, Unset, str]):
    """

    created_at: datetime.datetime
    csrf_token: str
    expires_at: datetime.datetime
    id: str
    user_id: str
    ip: Union[None, Unset, str] = UNSET
    user_agent: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        csrf_token = self.csrf_token

        expires_at = self.expires_at.isoformat()

        id = self.id

        user_id = self.user_id

        ip: Union[None, Unset, str]
        if isinstance(self.ip, Unset):
            ip = UNSET
        else:
            ip = self.ip

        user_agent: Union[None, Unset, str]
        if isinstance(self.user_agent, Unset):
            user_agent = UNSET
        else:
            user_agent = self.user_agent

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "csrf_token": csrf_token,
                "expires_at": expires_at,
                "id": id,
                "user_id": user_id,
            }
        )
        if ip is not UNSET:
            field_dict["ip"] = ip
        if user_agent is not UNSET:
            field_dict["user_agent"] = user_agent

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        csrf_token = d.pop("csrf_token")

        expires_at = isoparse(d.pop("expires_at"))

        id = d.pop("id")

        user_id = d.pop("user_id")

        def _parse_ip(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        ip = _parse_ip(d.pop("ip", UNSET))

        def _parse_user_agent(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        user_agent = _parse_user_agent(d.pop("user_agent", UNSET))

        session = cls(
            created_at=created_at,
            csrf_token=csrf_token,
            expires_at=expires_at,
            id=id,
            user_id=user_id,
            ip=ip,
            user_agent=user_agent,
        )

        session.additional_properties = d
        return session

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
