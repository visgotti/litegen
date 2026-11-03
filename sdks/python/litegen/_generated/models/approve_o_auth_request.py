from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ApproveOAuthRequest")


@_attrs_define
class ApproveOAuthRequest:
    """The person's approval.

    Attributes:
        org_id (str):
        scopes (List[str]): A subset of the requested scopes that the person's role can grant.
        app_id (Union[None, Unset, str]): The app the app acts in; the org's first app when omitted.
    """

    org_id: str
    scopes: List[str]
    app_id: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        org_id = self.org_id

        scopes = self.scopes

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org_id": org_id,
                "scopes": scopes,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        org_id = d.pop("org_id")

        scopes = cast(List[str], d.pop("scopes"))

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        approve_o_auth_request = cls(
            org_id=org_id,
            scopes=scopes,
            app_id=app_id,
        )

        approve_o_auth_request.additional_properties = d
        return approve_o_auth_request

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
