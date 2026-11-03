from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="OAuthClientView")


@_attrs_define
class OAuthClientView:
    """The app asking to sign in.

    Attributes:
        client_id (str):
        name (str):
        redirect_host (str): The host the browser is sent back to after the decision.
        registration (str): `cimd`: identified by its metadata document's URL. `dcr`: registered here.
        client_uri (Union[None, Unset, str]):
        logo_uri (Union[None, Unset, str]):
    """

    client_id: str
    name: str
    redirect_host: str
    registration: str
    client_uri: Union[None, Unset, str] = UNSET
    logo_uri: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        client_id = self.client_id

        name = self.name

        redirect_host = self.redirect_host

        registration = self.registration

        client_uri: Union[None, Unset, str]
        if isinstance(self.client_uri, Unset):
            client_uri = UNSET
        else:
            client_uri = self.client_uri

        logo_uri: Union[None, Unset, str]
        if isinstance(self.logo_uri, Unset):
            logo_uri = UNSET
        else:
            logo_uri = self.logo_uri

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "client_id": client_id,
                "name": name,
                "redirect_host": redirect_host,
                "registration": registration,
            }
        )
        if client_uri is not UNSET:
            field_dict["client_uri"] = client_uri
        if logo_uri is not UNSET:
            field_dict["logo_uri"] = logo_uri

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        client_id = d.pop("client_id")

        name = d.pop("name")

        redirect_host = d.pop("redirect_host")

        registration = d.pop("registration")

        def _parse_client_uri(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        client_uri = _parse_client_uri(d.pop("client_uri", UNSET))

        def _parse_logo_uri(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        logo_uri = _parse_logo_uri(d.pop("logo_uri", UNSET))

        o_auth_client_view = cls(
            client_id=client_id,
            name=name,
            redirect_host=redirect_host,
            registration=registration,
            client_uri=client_uri,
            logo_uri=logo_uri,
        )

        o_auth_client_view.additional_properties = d
        return o_auth_client_view

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
