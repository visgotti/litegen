from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.o_auth_app_choice import OAuthAppChoice


T = TypeVar("T", bound="OAuthOrgChoice")


@_attrs_define
class OAuthOrgChoice:
    """An org the person may approve the request in.

    Attributes:
        apps (List['OAuthAppChoice']):
        grantable_scopes (List[str]): The requested scopes the role can grant: what it could give an API key.
        id (str):
        name (str):
        role (str): The person's role in the org.
    """

    apps: List["OAuthAppChoice"]
    grantable_scopes: List[str]
    id: str
    name: str
    role: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        apps = []
        for apps_item_data in self.apps:
            apps_item = apps_item_data.to_dict()
            apps.append(apps_item)

        grantable_scopes = self.grantable_scopes

        id = self.id

        name = self.name

        role = self.role

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "apps": apps,
                "grantable_scopes": grantable_scopes,
                "id": id,
                "name": name,
                "role": role,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.o_auth_app_choice import OAuthAppChoice

        d = src_dict.copy()
        apps = []
        _apps = d.pop("apps")
        for apps_item_data in _apps:
            apps_item = OAuthAppChoice.from_dict(apps_item_data)

            apps.append(apps_item)

        grantable_scopes = cast(List[str], d.pop("grantable_scopes"))

        id = d.pop("id")

        name = d.pop("name")

        role = d.pop("role")

        o_auth_org_choice = cls(
            apps=apps,
            grantable_scopes=grantable_scopes,
            id=id,
            name=name,
            role=role,
        )

        o_auth_org_choice.additional_properties = d
        return o_auth_org_choice

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
