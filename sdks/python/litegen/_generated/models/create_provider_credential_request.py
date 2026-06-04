from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="CreateProviderCredentialRequest")


@_attrs_define
class CreateProviderCredentialRequest:
    """
    Attributes:
        credentials (Any): The provider's secret fields, e.g. `{"api_key":"sk-..."}`.
        provider (str):
    """

    credentials: Any
    provider: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        credentials = self.credentials

        provider = self.provider

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "credentials": credentials,
                "provider": provider,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        credentials = d.pop("credentials")

        provider = d.pop("provider")

        create_provider_credential_request = cls(
            credentials=credentials,
            provider=provider,
        )

        create_provider_credential_request.additional_properties = d
        return create_provider_credential_request

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
