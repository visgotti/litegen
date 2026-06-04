from typing import Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="AuthConfigResponse")


@_attrs_define
class AuthConfigResponse:
    """
    Attributes:
        password_enabled (bool): Whether email/password signup + login is enabled.
        providers_enabled (List[str]): OAuth providers with both CLIENT_ID and CLIENT_SECRET configured
            (e.g. `["github", "google"]`).
        signup_open (bool): Whether self-service signup is open (true in hosted mode).
    """

    password_enabled: bool
    providers_enabled: List[str]
    signup_open: bool
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        password_enabled = self.password_enabled

        providers_enabled = self.providers_enabled

        signup_open = self.signup_open

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "password_enabled": password_enabled,
                "providers_enabled": providers_enabled,
                "signup_open": signup_open,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        password_enabled = d.pop("password_enabled")

        providers_enabled = cast(List[str], d.pop("providers_enabled"))

        signup_open = d.pop("signup_open")

        auth_config_response = cls(
            password_enabled=password_enabled,
            providers_enabled=providers_enabled,
            signup_open=signup_open,
        )

        auth_config_response.additional_properties = d
        return auth_config_response

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
