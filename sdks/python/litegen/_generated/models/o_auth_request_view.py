import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

if TYPE_CHECKING:
    from ..models.o_auth_client_view import OAuthClientView
    from ..models.o_auth_org_choice import OAuthOrgChoice


T = TypeVar("T", bound="OAuthRequestView")


@_attrs_define
class OAuthRequestView:
    """A pending sign-in request, as the consent page shows it.

    Attributes:
        client (OAuthClientView): The app asking to sign in.
        expires_at (datetime.datetime):
        id (str):
        orgs (List['OAuthOrgChoice']): The person's orgs, each with its apps and the requested scopes their
            role there lets them grant.
        requested_scopes (List[str]): The scopes the app asked for, all of one endpoint.
        surface (str): The endpoint the grant would be for: `ai` (`/mcp`) or `admin` (`/admin/mcp`).
    """

    client: "OAuthClientView"
    expires_at: datetime.datetime
    id: str
    orgs: List["OAuthOrgChoice"]
    requested_scopes: List[str]
    surface: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        client = self.client.to_dict()

        expires_at = self.expires_at.isoformat()

        id = self.id

        orgs = []
        for orgs_item_data in self.orgs:
            orgs_item = orgs_item_data.to_dict()
            orgs.append(orgs_item)

        requested_scopes = self.requested_scopes

        surface = self.surface

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "client": client,
                "expires_at": expires_at,
                "id": id,
                "orgs": orgs,
                "requested_scopes": requested_scopes,
                "surface": surface,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.o_auth_client_view import OAuthClientView
        from ..models.o_auth_org_choice import OAuthOrgChoice

        d = src_dict.copy()
        client = OAuthClientView.from_dict(d.pop("client"))

        expires_at = isoparse(d.pop("expires_at"))

        id = d.pop("id")

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = OAuthOrgChoice.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        requested_scopes = cast(List[str], d.pop("requested_scopes"))

        surface = d.pop("surface")

        o_auth_request_view = cls(
            client=client,
            expires_at=expires_at,
            id=id,
            orgs=orgs,
            requested_scopes=requested_scopes,
            surface=surface,
        )

        o_auth_request_view.additional_properties = d
        return o_auth_request_view

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
