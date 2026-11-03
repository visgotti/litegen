from typing import (
    TYPE_CHECKING,
    Any,
    Dict,
    List,
    Type,
    TypeVar,
    Union,
    cast,
)

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.mcp_endpoints import McpEndpoints


T = TypeVar("T", bound="AuthConfigResponse")


@_attrs_define
class AuthConfigResponse:
    """
    Attributes:
        password_enabled (bool): Whether email/password signup + login is enabled.
        providers_enabled (List[str]): OAuth providers with both CLIENT_ID and CLIENT_SECRET configured
            (e.g. `["github", "google"]`).
        signup_open (bool): Whether self-service signup is open (true in hosted mode).
        mcp (Union['McpEndpoints', None, Unset]):
    """

    password_enabled: bool
    providers_enabled: List[str]
    signup_open: bool
    mcp: Union["McpEndpoints", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.mcp_endpoints import McpEndpoints

        password_enabled = self.password_enabled

        providers_enabled = self.providers_enabled

        signup_open = self.signup_open

        mcp: Union[Dict[str, Any], None, Unset]
        if isinstance(self.mcp, Unset):
            mcp = UNSET
        elif isinstance(self.mcp, McpEndpoints):
            mcp = self.mcp.to_dict()
        else:
            mcp = self.mcp

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "password_enabled": password_enabled,
                "providers_enabled": providers_enabled,
                "signup_open": signup_open,
            }
        )
        if mcp is not UNSET:
            field_dict["mcp"] = mcp

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.mcp_endpoints import McpEndpoints

        d = src_dict.copy()
        password_enabled = d.pop("password_enabled")

        providers_enabled = cast(List[str], d.pop("providers_enabled"))

        signup_open = d.pop("signup_open")

        def _parse_mcp(data: object) -> Union["McpEndpoints", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                mcp_type_1 = McpEndpoints.from_dict(data)

                return mcp_type_1
            except:  # noqa: E722
                pass
            return cast(Union["McpEndpoints", None, Unset], data)

        mcp = _parse_mcp(d.pop("mcp", UNSET))

        auth_config_response = cls(
            password_enabled=password_enabled,
            providers_enabled=providers_enabled,
            signup_open=signup_open,
            mcp=mcp,
        )

        auth_config_response.additional_properties = d
        return auth_config_response

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
