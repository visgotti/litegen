from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="OrgTransferOwnerRequest")


@_attrs_define
class OrgTransferOwnerRequest:
    """Renamed in the OpenAPI schema to `OrgTransferOwnerRequest` to avoid a
    component name collision with `users::TransferOwnerRequest`.

        Attributes:
            new_owner_user_id (str):
    """

    new_owner_user_id: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        new_owner_user_id = self.new_owner_user_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "new_owner_user_id": new_owner_user_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        new_owner_user_id = d.pop("new_owner_user_id")

        org_transfer_owner_request = cls(
            new_owner_user_id=new_owner_user_id,
        )

        org_transfer_owner_request.additional_properties = d
        return org_transfer_owner_request

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
