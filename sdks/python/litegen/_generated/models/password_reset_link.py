from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="PasswordResetLink")


@_attrs_define
class PasswordResetLink:
    """A password reset link issued for an account. Shown only in this answer:
    litegen sends no email, so give `reset_url` or `token` to the account's
    owner.

        Attributes:
            email (str):
            expires_at (str):
            reset_url (Union[None, str]): The dashboard's page that sets the password: `/reset-password/{token}`
                on the origin of `server.public_base_url`; null when it isn't set:
                build it from `token` on the dashboard's origin.
            token (str): Sets the account's password once, until `expires_at` (1 hour), with
                `POST /v1/auth/password-reset/confirm`. Whoever holds it can take over
                the account, so give it only to its owner.
            user_id (str):
    """

    email: str
    expires_at: str
    reset_url: Union[None, str]
    token: str
    user_id: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        email = self.email

        expires_at = self.expires_at

        reset_url: Union[None, str]
        reset_url = self.reset_url

        token = self.token

        user_id = self.user_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "email": email,
                "expires_at": expires_at,
                "reset_url": reset_url,
                "token": token,
                "user_id": user_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        email = d.pop("email")

        expires_at = d.pop("expires_at")

        def _parse_reset_url(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        reset_url = _parse_reset_url(d.pop("reset_url"))

        token = d.pop("token")

        user_id = d.pop("user_id")

        password_reset_link = cls(
            email=email,
            expires_at=expires_at,
            reset_url=reset_url,
            token=token,
            user_id=user_id,
        )

        password_reset_link.additional_properties = d
        return password_reset_link

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
