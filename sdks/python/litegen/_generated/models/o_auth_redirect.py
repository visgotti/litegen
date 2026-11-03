from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="OAuthRedirect")


@_attrs_define
class OAuthRedirect:
    """Where to send the browser next.

    Attributes:
        redirect_to (str): The app's redirect URI, carrying the code or the error, `state` and `iss`.
    """

    redirect_to: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        redirect_to = self.redirect_to

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "redirect_to": redirect_to,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        redirect_to = d.pop("redirect_to")

        o_auth_redirect = cls(
            redirect_to=redirect_to,
        )

        o_auth_redirect.additional_properties = d
        return o_auth_redirect

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
