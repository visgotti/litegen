from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="McpEndpoints")


@_attrs_define
class McpEndpoints:
    """The MCP endpoints a deployment serves, for the dashboard's Connect page.

    Attributes:
        ai_url (str): The AI endpoint: `{public_base_url}/mcp`.
        oauth (bool): Whether clients can sign in with OAuth (ChatGPT and claude.ai need it).
        admin_url (Union[None, Unset, str]): The management endpoint, `{public_base_url}/admin/mcp`, when it is served.
    """

    ai_url: str
    oauth: bool
    admin_url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        ai_url = self.ai_url

        oauth = self.oauth

        admin_url: Union[None, Unset, str]
        if isinstance(self.admin_url, Unset):
            admin_url = UNSET
        else:
            admin_url = self.admin_url

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "ai_url": ai_url,
                "oauth": oauth,
            }
        )
        if admin_url is not UNSET:
            field_dict["admin_url"] = admin_url

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        ai_url = d.pop("ai_url")

        oauth = d.pop("oauth")

        def _parse_admin_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        admin_url = _parse_admin_url(d.pop("admin_url", UNSET))

        mcp_endpoints = cls(
            ai_url=ai_url,
            oauth=oauth,
            admin_url=admin_url,
        )

        mcp_endpoints.additional_properties = d
        return mcp_endpoints

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
