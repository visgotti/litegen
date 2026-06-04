import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="ProviderCredentialInfo")


@_attrs_define
class ProviderCredentialInfo:
    """Public view of a stored BYO provider credential — NEVER the plaintext secret.

    Attributes:
        created_at (datetime.datetime):
        provider (str):
        display_hint (Union[None, Unset, str]):
    """

    created_at: datetime.datetime
    provider: str
    display_hint: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        provider = self.provider

        display_hint: Union[None, Unset, str]
        if isinstance(self.display_hint, Unset):
            display_hint = UNSET
        else:
            display_hint = self.display_hint

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "provider": provider,
            }
        )
        if display_hint is not UNSET:
            field_dict["display_hint"] = display_hint

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        provider = d.pop("provider")

        def _parse_display_hint(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        display_hint = _parse_display_hint(d.pop("display_hint", UNSET))

        provider_credential_info = cls(
            created_at=created_at,
            provider=provider,
            display_hint=display_hint,
        )

        provider_credential_info.additional_properties = d
        return provider_credential_info

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
