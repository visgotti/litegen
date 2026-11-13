from typing import Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="MeOrg")


@_attrs_define
class MeOrg:
    """One organization the person belongs to.

    Attributes:
        id (str):
        name (str):
        permissions (List[str]): The permissions this role holds acting in this organization, as the
            server checks them on every request (e.g. `org:write`,
            `generation:create`), for a client to offer only what will work. The
            deployment's own permissions (`system:config`, `cache:clear`,
            `user:read:any`, `user:write:any`, `user:delete:any`,
            `system:transfer_owner`) are listed only for the default org of a
            single-tenant deployment, and in hosted mode never. A sign-in through
            OAuth is also held to its scopes.
        role (str): The person's role in this organization: `owner`, `admin`, `member` or
            `viewer`.
    """

    id: str
    name: str
    permissions: List[str]
    role: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        id = self.id

        name = self.name

        permissions = self.permissions

        role = self.role

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
                "name": name,
                "permissions": permissions,
                "role": role,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        id = d.pop("id")

        name = d.pop("name")

        permissions = cast(List[str], d.pop("permissions"))

        role = d.pop("role")

        me_org = cls(
            id=id,
            name=name,
            permissions=permissions,
            role=role,
        )

        me_org.additional_properties = d
        return me_org

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
