from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="CredentialEntry")


@_attrs_define
class CredentialEntry:
    """A signing credential set (key_id + key_secret, plus optional region for
    SigV4/TC3) with a weight for round-robin distribution. The signing-scheme
    analogue of [`ApiKeyEntry`], used by providers like Bedrock, Hunyuan, and
    Kling that authenticate with a credential *pair* rather than a bearer key.

        Attributes:
            key_id (str): Access key id (SigV4) / secret id (TC3) / access key (Kling).
            key_secret (str): Secret access key (SigV4) / secret key (TC3, Kling). Stored encrypted.
            label (Union[None, Unset, str]): Optional label for the credential set.
            region (Union[None, Unset, str]): Region (SigV4 / TC3). Omitted for Kling; when absent the provider falls
                back to its configured/default region.
            weight (Union[Unset, int]): Weight for round-robin (higher = more traffic). Default: 1.
    """

    key_id: str
    key_secret: str
    label: Union[None, Unset, str] = UNSET
    region: Union[None, Unset, str] = UNSET
    weight: Union[Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        key_id = self.key_id

        key_secret = self.key_secret

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        region: Union[None, Unset, str]
        if isinstance(self.region, Unset):
            region = UNSET
        else:
            region = self.region

        weight = self.weight

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "key_id": key_id,
                "key_secret": key_secret,
            }
        )
        if label is not UNSET:
            field_dict["label"] = label
        if region is not UNSET:
            field_dict["region"] = region
        if weight is not UNSET:
            field_dict["weight"] = weight

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        key_id = d.pop("key_id")

        key_secret = d.pop("key_secret")

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_region(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        region = _parse_region(d.pop("region", UNSET))

        weight = d.pop("weight", UNSET)

        credential_entry = cls(
            key_id=key_id,
            key_secret=key_secret,
            label=label,
            region=region,
            weight=weight,
        )

        credential_entry.additional_properties = d
        return credential_entry

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
