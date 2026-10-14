from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.provider_settings_extra_headers_type_0 import (
        ProviderSettingsExtraHeadersType0,
    )
    from ..models.provider_settings_model_mapping_type_0 import (
        ProviderSettingsModelMappingType0,
    )


T = TypeVar("T", bound="ProviderSettings")


@_attrs_define
class ProviderSettings:
    """Non-secret part of a runtime provider config. Unset fields fall back to the
    provider's env/YAML config.

        Attributes:
            api_base (Union[None, Unset, str]):
            enabled (Union[None, Unset, bool]): False takes the provider out of service until re-enabled.
            extra_headers (Union['ProviderSettingsExtraHeadersType0', None, Unset]):
            model_mapping (Union['ProviderSettingsModelMappingType0', None, Unset]):
            options (Union[Unset, Any]):
            region (Union[None, Unset, str]):
    """

    api_base: Union[None, Unset, str] = UNSET
    enabled: Union[None, Unset, bool] = UNSET
    extra_headers: Union["ProviderSettingsExtraHeadersType0", None, Unset] = UNSET
    model_mapping: Union["ProviderSettingsModelMappingType0", None, Unset] = UNSET
    options: Union[Unset, Any] = UNSET
    region: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.provider_settings_extra_headers_type_0 import (
            ProviderSettingsExtraHeadersType0,
        )
        from ..models.provider_settings_model_mapping_type_0 import (
            ProviderSettingsModelMappingType0,
        )

        api_base: Union[None, Unset, str]
        if isinstance(self.api_base, Unset):
            api_base = UNSET
        else:
            api_base = self.api_base

        enabled: Union[None, Unset, bool]
        if isinstance(self.enabled, Unset):
            enabled = UNSET
        else:
            enabled = self.enabled

        extra_headers: Union[Dict[str, Any], None, Unset]
        if isinstance(self.extra_headers, Unset):
            extra_headers = UNSET
        elif isinstance(self.extra_headers, ProviderSettingsExtraHeadersType0):
            extra_headers = self.extra_headers.to_dict()
        else:
            extra_headers = self.extra_headers

        model_mapping: Union[Dict[str, Any], None, Unset]
        if isinstance(self.model_mapping, Unset):
            model_mapping = UNSET
        elif isinstance(self.model_mapping, ProviderSettingsModelMappingType0):
            model_mapping = self.model_mapping.to_dict()
        else:
            model_mapping = self.model_mapping

        options = self.options

        region: Union[None, Unset, str]
        if isinstance(self.region, Unset):
            region = UNSET
        else:
            region = self.region

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
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
        if region is not UNSET:
            field_dict["region"] = region

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.provider_settings_extra_headers_type_0 import (
            ProviderSettingsExtraHeadersType0,
        )
        from ..models.provider_settings_model_mapping_type_0 import (
            ProviderSettingsModelMappingType0,
        )

        d = src_dict.copy()

        def _parse_api_base(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        api_base = _parse_api_base(d.pop("api_base", UNSET))

        def _parse_enabled(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        enabled = _parse_enabled(d.pop("enabled", UNSET))

        def _parse_extra_headers(
            data: object,
        ) -> Union["ProviderSettingsExtraHeadersType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                extra_headers_type_0 = ProviderSettingsExtraHeadersType0.from_dict(data)

                return extra_headers_type_0
            except:  # noqa: E722
                pass
            return cast(Union["ProviderSettingsExtraHeadersType0", None, Unset], data)

        extra_headers = _parse_extra_headers(d.pop("extra_headers", UNSET))

        def _parse_model_mapping(
            data: object,
        ) -> Union["ProviderSettingsModelMappingType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                model_mapping_type_0 = ProviderSettingsModelMappingType0.from_dict(data)

                return model_mapping_type_0
            except:  # noqa: E722
                pass
            return cast(Union["ProviderSettingsModelMappingType0", None, Unset], data)

        model_mapping = _parse_model_mapping(d.pop("model_mapping", UNSET))

        options = d.pop("options", UNSET)

        def _parse_region(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        region = _parse_region(d.pop("region", UNSET))

        provider_settings = cls(
            api_base=api_base,
            enabled=enabled,
            extra_headers=extra_headers,
            model_mapping=model_mapping,
            options=options,
            region=region,
        )

        provider_settings.additional_properties = d
        return provider_settings

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
