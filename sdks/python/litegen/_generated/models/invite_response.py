from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="InviteResponse")


@_attrs_define
class InviteResponse:
    """A new invitation. Its `token` and `invite_url` are shown only in this
    answer: litegen sends no email, so give one of them to the invitee.

        Attributes:
            email (str):
            expires_at (str):
            id (str):
            invite_url (Union[None, str]): The dashboard's page that accepts it: `/invite/{token}` on the origin
                of `server.public_base_url`; null when it isn't set: build it from
                `token` on the dashboard's origin.
            org_id (str): The organization the invitee joins on accepting.
            role (str):
            token (str): Accepts the invitation once, until `expires_at`
                (`POST /v1/auth/invitations/{token}/accept`): whoever holds it joins
                the org as `role`, so give it only to the invitee.
    """

    email: str
    expires_at: str
    id: str
    invite_url: Union[None, str]
    org_id: str
    role: str
    token: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        email = self.email

        expires_at = self.expires_at

        id = self.id

        invite_url: Union[None, str]
        invite_url = self.invite_url

        org_id = self.org_id

        role = self.role

        token = self.token

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "email": email,
                "expires_at": expires_at,
                "id": id,
                "invite_url": invite_url,
                "org_id": org_id,
                "role": role,
                "token": token,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        email = d.pop("email")

        expires_at = d.pop("expires_at")

        id = d.pop("id")

        def _parse_invite_url(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        invite_url = _parse_invite_url(d.pop("invite_url"))

        org_id = d.pop("org_id")

        role = d.pop("role")

        token = d.pop("token")

        invite_response = cls(
            email=email,
            expires_at=expires_at,
            id=id,
            invite_url=invite_url,
            org_id=org_id,
            role=role,
            token=token,
        )

        invite_response.additional_properties = d
        return invite_response

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
