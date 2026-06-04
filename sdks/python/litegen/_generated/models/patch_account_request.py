from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PatchAccountRequest")


@_attrs_define
class PatchAccountRequest:
    """
    Attributes:
        current_password (Union[None, Unset, str]):
        new_password (Union[None, Unset, str]):
    """

    current_password: Union[None, Unset, str] = UNSET
    new_password: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        current_password: Union[None, Unset, str]
        if isinstance(self.current_password, Unset):
            current_password = UNSET
        else:
            current_password = self.current_password

        new_password: Union[None, Unset, str]
        if isinstance(self.new_password, Unset):
            new_password = UNSET
        else:
            new_password = self.new_password

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if current_password is not UNSET:
            field_dict["current_password"] = current_password
        if new_password is not UNSET:
            field_dict["new_password"] = new_password

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_current_password(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        current_password = _parse_current_password(d.pop("current_password", UNSET))

        def _parse_new_password(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        new_password = _parse_new_password(d.pop("new_password", UNSET))

        patch_account_request = cls(
            current_password=current_password,
            new_password=new_password,
        )

        patch_account_request.additional_properties = d
        return patch_account_request

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
