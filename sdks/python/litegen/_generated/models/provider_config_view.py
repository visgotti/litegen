import datetime
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
from dateutil.parser import isoparse

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.provider_config_view_model_mapping import (
        ProviderConfigViewModelMapping,
    )


T = TypeVar("T", bound="ProviderConfigView")


@_attrs_define
class ProviderConfigView:
    """Admin view of one provider: what is configured, with every secret masked.

    Attributes:
        active (bool): Whether a live instance is registered and serving requests.
        enabled (bool):
        extra_header_names (List[str]): Header names only; values can carry secrets.
        has_secret (bool):
        modalities (List[str]): Media this adapter can serve: image, video.
        model_mapping (ProviderConfigViewModelMapping):
        name (str):
        source (str): Where the effective config comes from: `env`, `runtime` or `none`.
        api_base (Union[None, Unset, str]):
        key_hint (Union[None, Unset, str]): Last 4 characters of the effective key, e.g. `…a1b2`. Never the secret.
        options (Union[Unset, Any]):
        region (Union[None, Unset, str]):
        updated_at (Union[None, Unset, datetime.datetime]):
    """

    active: bool
    enabled: bool
    extra_header_names: List[str]
    has_secret: bool
    modalities: List[str]
    model_mapping: "ProviderConfigViewModelMapping"
    name: str
    source: str
    api_base: Union[None, Unset, str] = UNSET
    key_hint: Union[None, Unset, str] = UNSET
    options: Union[Unset, Any] = UNSET
    region: Union[None, Unset, str] = UNSET
    updated_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        active = self.active

        enabled = self.enabled

        extra_header_names = self.extra_header_names

        has_secret = self.has_secret

        modalities = self.modalities

        model_mapping = self.model_mapping.to_dict()

        name = self.name

        source = self.source

        api_base: Union[None, Unset, str]
        if isinstance(self.api_base, Unset):
            api_base = UNSET
        else:
            api_base = self.api_base

        key_hint: Union[None, Unset, str]
        if isinstance(self.key_hint, Unset):
            key_hint = UNSET
        else:
            key_hint = self.key_hint

        options = self.options

        region: Union[None, Unset, str]
        if isinstance(self.region, Unset):
            region = UNSET
        else:
            region = self.region

        updated_at: Union[None, Unset, str]
        if isinstance(self.updated_at, Unset):
            updated_at = UNSET
        elif isinstance(self.updated_at, datetime.datetime):
            updated_at = self.updated_at.isoformat()
        else:
            updated_at = self.updated_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active": active,
                "enabled": enabled,
                "extra_header_names": extra_header_names,
                "has_secret": has_secret,
                "modalities": modalities,
                "model_mapping": model_mapping,
                "name": name,
                "source": source,
            }
        )
        if api_base is not UNSET:
            field_dict["api_base"] = api_base
        if key_hint is not UNSET:
            field_dict["key_hint"] = key_hint
        if options is not UNSET:
            field_dict["options"] = options
        if region is not UNSET:
            field_dict["region"] = region
        if updated_at is not UNSET:
            field_dict["updated_at"] = updated_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.provider_config_view_model_mapping import (
            ProviderConfigViewModelMapping,
        )

        d = src_dict.copy()
        active = d.pop("active")

        enabled = d.pop("enabled")

        extra_header_names = cast(List[str], d.pop("extra_header_names"))

        has_secret = d.pop("has_secret")

        modalities = cast(List[str], d.pop("modalities"))

        model_mapping = ProviderConfigViewModelMapping.from_dict(d.pop("model_mapping"))

        name = d.pop("name")

        source = d.pop("source")

        def _parse_api_base(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        api_base = _parse_api_base(d.pop("api_base", UNSET))

        def _parse_key_hint(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        key_hint = _parse_key_hint(d.pop("key_hint", UNSET))

        options = d.pop("options", UNSET)

        def _parse_region(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        region = _parse_region(d.pop("region", UNSET))

        def _parse_updated_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                updated_at_type_0 = isoparse(data)

                return updated_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        updated_at = _parse_updated_at(d.pop("updated_at", UNSET))

        provider_config_view = cls(
            active=active,
            enabled=enabled,
            extra_header_names=extra_header_names,
            has_secret=has_secret,
            modalities=modalities,
            model_mapping=model_mapping,
            name=name,
            source=source,
            api_base=api_base,
            key_hint=key_hint,
            options=options,
            region=region,
            updated_at=updated_at,
        )

        provider_config_view.additional_properties = d
        return provider_config_view

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
