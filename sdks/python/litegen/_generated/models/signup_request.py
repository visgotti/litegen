from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SignupRequest")


@_attrs_define
class SignupRequest:
    """
    Attributes:
        email (str):
        password (str):
        org_name (Union[None, Unset, str]): Optional organization name (hosted mode). Defaults to the email local-part.
    """

    email: str
    password: str
    org_name: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        email = self.email

        password = self.password

        org_name: Union[None, Unset, str]
        if isinstance(self.org_name, Unset):
            org_name = UNSET
        else:
            org_name = self.org_name

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "email": email,
                "password": password,
            }
        )
        if org_name is not UNSET:
            field_dict["org_name"] = org_name

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        email = d.pop("email")

        password = d.pop("password")

        def _parse_org_name(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_name = _parse_org_name(d.pop("org_name", UNSET))

        signup_request = cls(
            email=email,
            password=password,
            org_name=org_name,
        )

        signup_request.additional_properties = d
        return signup_request

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
