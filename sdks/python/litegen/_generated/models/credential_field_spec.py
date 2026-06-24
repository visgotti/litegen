from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CredentialFieldSpec")


@_attrs_define
class CredentialFieldSpec:
    """One field the dashboard must collect for a provider credential, per pool
    entry. Returned as part of [`ProviderCatalogEntry`].

        Attributes:
            key (str): JSON key within each pool entry (e.g. "key", "key_id", "key_secret", "region").
            label (str): Human-readable label for the input.
            secret (bool): Whether the value is secret and should be rendered masked.
            optional (Union[Unset, bool]): Whether the field may be left empty.
    """

    key: str
    label: str
    secret: bool
    optional: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        key = self.key

        label = self.label

        secret = self.secret

        optional = self.optional

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key": key,
                "label": label,
                "secret": secret,
            }
        )
        if optional is not UNSET:
            field_dict["optional"] = optional

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        key = d.pop("key")

        label = d.pop("label")

        secret = d.pop("secret")

        optional = d.pop("optional", UNSET)

        credential_field_spec = cls(
            key=key,
            label=label,
            secret=secret,
            optional=optional,
        )

        credential_field_spec.additional_properties = d
        return credential_field_spec

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
