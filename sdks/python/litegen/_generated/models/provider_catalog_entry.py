from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.credential_field_spec import CredentialFieldSpec


T = TypeVar("T", bound="ProviderCatalogEntry")


@_attrs_define
class ProviderCatalogEntry:
    """Describes a provider and how the dashboard should render its credential
    form. Returned by `GET /v1/providers` so the UI never hard-codes per-provider
    field knowledge.

        Attributes:
            auth_scheme (str): Coarse auth scheme: "api_key" | "aws_sigv4" | "tencent_tc3" | "kling_jwt".
            fields (List['CredentialFieldSpec']): Fields to collect per pool entry, besides the universal `weight`/`label`.
            modalities (List[str]): Which media this provider serves — any of "image", "video".
            name (str): Provider id (e.g. "openai", "bedrock").
            pool_field (str): Which credentials array to submit: "api_keys" (bearer) or
                "credential_sets" (signing). Each array element is `{<fields…>, weight?, label?}`.
    """

    auth_scheme: str
    fields: List["CredentialFieldSpec"]
    modalities: List[str]
    name: str
    pool_field: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        auth_scheme = self.auth_scheme

        fields = []
        for fields_item_data in self.fields:
            fields_item = fields_item_data.to_dict()
            fields.append(fields_item)

        modalities = self.modalities

        name = self.name

        pool_field = self.pool_field

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "auth_scheme": auth_scheme,
                "fields": fields,
                "modalities": modalities,
                "name": name,
                "pool_field": pool_field,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.credential_field_spec import CredentialFieldSpec

        d = src_dict.copy()
        auth_scheme = d.pop("auth_scheme")

        fields = []
        _fields = d.pop("fields")
        for fields_item_data in _fields:
            fields_item = CredentialFieldSpec.from_dict(fields_item_data)

            fields.append(fields_item)

        modalities = cast(List[str], d.pop("modalities"))

        name = d.pop("name")

        pool_field = d.pop("pool_field")

        provider_catalog_entry = cls(
            auth_scheme=auth_scheme,
            fields=fields,
            modalities=modalities,
            name=name,
            pool_field=pool_field,
        )

        provider_catalog_entry.additional_properties = d
        return provider_catalog_entry

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
