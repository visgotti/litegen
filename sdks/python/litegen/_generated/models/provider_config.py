from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.api_key_entry import ApiKeyEntry
    from ..models.provider_config_extra_headers import ProviderConfigExtraHeaders
    from ..models.provider_config_model_mapping import ProviderConfigModelMapping


T = TypeVar("T", bound="ProviderConfig")


@_attrs_define
class ProviderConfig:
    """Configuration for a provider deployment.

    Attributes:
        api_keys (List['ApiKeyEntry']): API key(s) with optional weights.
        provider (str): Provider name (e.g. "openai", "stability", "replicate").
        api_base (Union[None, Unset, str]): Base URL override.
        enabled (Union[Unset, bool]): Whether this provider config is enabled.
        extra_headers (Union[Unset, ProviderConfigExtraHeaders]): Extra headers.
        model_mapping (Union[Unset, ProviderConfigModelMapping]): Model ID mapping: internal ID → provider's model ID.
        options (Union[Unset, Any]): Provider-specific options.
    """

    api_keys: List["ApiKeyEntry"]
    provider: str
    api_base: Union[None, Unset, str] = UNSET
    enabled: Union[Unset, bool] = UNSET
    extra_headers: Union[Unset, "ProviderConfigExtraHeaders"] = UNSET
    model_mapping: Union[Unset, "ProviderConfigModelMapping"] = UNSET
    options: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        api_keys = []
        for api_keys_item_data in self.api_keys:
            api_keys_item = api_keys_item_data.to_dict()
            api_keys.append(api_keys_item)

        provider = self.provider

        api_base: Union[None, Unset, str]
        if isinstance(self.api_base, Unset):
            api_base = UNSET
        else:
            api_base = self.api_base

        enabled = self.enabled

        extra_headers: Union[Unset, Dict[str, Any]] = UNSET
        if not isinstance(self.extra_headers, Unset):
            extra_headers = self.extra_headers.to_dict()

        model_mapping: Union[Unset, Dict[str, Any]] = UNSET
        if not isinstance(self.model_mapping, Unset):
            model_mapping = self.model_mapping.to_dict()

        options = self.options

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "api_keys": api_keys,
                "provider": provider,
            }
        )
        if api_base is not UNSET:
            field_dict["api_base"] = api_base
        if enabled is not UNSET:
            field_dict["enabled"] = enabled
        if extra_headers is not UNSET:
            field_dict["extra_headers"] = extra_headers
        if model_mapping is not UNSET:
            field_dict["model_mapping"] = model_mapping
        if options is not UNSET:
            field_dict["options"] = options

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.api_key_entry import ApiKeyEntry
        from ..models.provider_config_extra_headers import ProviderConfigExtraHeaders
        from ..models.provider_config_model_mapping import ProviderConfigModelMapping

        d = src_dict.copy()
        api_keys = []
        _api_keys = d.pop("api_keys")
        for api_keys_item_data in _api_keys:
            api_keys_item = ApiKeyEntry.from_dict(api_keys_item_data)

            api_keys.append(api_keys_item)

        provider = d.pop("provider")

        def _parse_api_base(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        api_base = _parse_api_base(d.pop("api_base", UNSET))

        enabled = d.pop("enabled", UNSET)

        _extra_headers = d.pop("extra_headers", UNSET)
        extra_headers: Union[Unset, ProviderConfigExtraHeaders]
        if isinstance(_extra_headers, Unset):
            extra_headers = UNSET
        else:
            extra_headers = ProviderConfigExtraHeaders.from_dict(_extra_headers)

        _model_mapping = d.pop("model_mapping", UNSET)
        model_mapping: Union[Unset, ProviderConfigModelMapping]
        if isinstance(_model_mapping, Unset):
            model_mapping = UNSET
        else:
            model_mapping = ProviderConfigModelMapping.from_dict(_model_mapping)

        options = d.pop("options", UNSET)

        provider_config = cls(
            api_keys=api_keys,
            provider=provider,
            api_base=api_base,
            enabled=enabled,
            extra_headers=extra_headers,
            model_mapping=model_mapping,
            options=options,
        )

        provider_config.additional_properties = d
        return provider_config

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
