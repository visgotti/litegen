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

from ..models.capability_media_type import CapabilityMediaType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.capability_model_pricing import CapabilityModelPricing
    from ..models.model_capability_flags import ModelCapabilityFlags
    from ..models.model_schema_params import ModelSchemaParams
    from ..models.prompt_spec import PromptSpec
    from ..models.ref_input_spec import RefInputSpec


T = TypeVar("T", bound="ModelSchema")


@_attrs_define
class ModelSchema:
    """
    Attributes:
        capabilities (ModelCapabilityFlags):
        display_name (str):
        id (str):
        media_type (CapabilityMediaType):
        pricing (CapabilityModelPricing):
        prompt (PromptSpec):
        provider (str):
        description (Union[Unset, str]):
        extra_allowlist (Union[Unset, List[str]]):
        params (Union[Unset, ModelSchemaParams]):
        ref_inputs (Union['RefInputSpec', None, Unset]):
        tags (Union[Unset, List[str]]):
    """

    capabilities: "ModelCapabilityFlags"
    display_name: str
    id: str
    media_type: CapabilityMediaType
    pricing: "CapabilityModelPricing"
    prompt: "PromptSpec"
    provider: str
    description: Union[Unset, str] = UNSET
    extra_allowlist: Union[Unset, List[str]] = UNSET
    params: Union[Unset, "ModelSchemaParams"] = UNSET
    ref_inputs: Union["RefInputSpec", None, Unset] = UNSET
    tags: Union[Unset, List[str]] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.ref_input_spec import RefInputSpec

        capabilities = self.capabilities.to_dict()

        display_name = self.display_name

        id = self.id

        media_type = self.media_type.value

        pricing = self.pricing.to_dict()

        prompt = self.prompt.to_dict()

        provider = self.provider

        description = self.description

        extra_allowlist: Union[Unset, List[str]] = UNSET
        if not isinstance(self.extra_allowlist, Unset):
            extra_allowlist = self.extra_allowlist

        params: Union[Unset, Dict[str, Any]] = UNSET
        if not isinstance(self.params, Unset):
            params = self.params.to_dict()

        ref_inputs: Union[Dict[str, Any], None, Unset]
        if isinstance(self.ref_inputs, Unset):
            ref_inputs = UNSET
        elif isinstance(self.ref_inputs, RefInputSpec):
            ref_inputs = self.ref_inputs.to_dict()
        else:
            ref_inputs = self.ref_inputs

        tags: Union[Unset, List[str]] = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "capabilities": capabilities,
                "display_name": display_name,
                "id": id,
                "media_type": media_type,
                "pricing": pricing,
                "prompt": prompt,
                "provider": provider,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if extra_allowlist is not UNSET:
            field_dict["extra_allowlist"] = extra_allowlist
        if params is not UNSET:
            field_dict["params"] = params
        if ref_inputs is not UNSET:
            field_dict["ref_inputs"] = ref_inputs
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.capability_model_pricing import CapabilityModelPricing
        from ..models.model_capability_flags import ModelCapabilityFlags
        from ..models.model_schema_params import ModelSchemaParams
        from ..models.prompt_spec import PromptSpec
        from ..models.ref_input_spec import RefInputSpec

        d = src_dict.copy()
        capabilities = ModelCapabilityFlags.from_dict(d.pop("capabilities"))

        display_name = d.pop("display_name")

        id = d.pop("id")

        media_type = CapabilityMediaType(d.pop("media_type"))

        pricing = CapabilityModelPricing.from_dict(d.pop("pricing"))

        prompt = PromptSpec.from_dict(d.pop("prompt"))

        provider = d.pop("provider")

        description = d.pop("description", UNSET)

        extra_allowlist = cast(List[str], d.pop("extra_allowlist", UNSET))

        _params = d.pop("params", UNSET)
        params: Union[Unset, ModelSchemaParams]
        if isinstance(_params, Unset):
            params = UNSET
        else:
            params = ModelSchemaParams.from_dict(_params)

        def _parse_ref_inputs(data: object) -> Union["RefInputSpec", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                ref_inputs_type_1 = RefInputSpec.from_dict(data)

                return ref_inputs_type_1
            except:  # noqa: E722
                pass
            return cast(Union["RefInputSpec", None, Unset], data)

        ref_inputs = _parse_ref_inputs(d.pop("ref_inputs", UNSET))

        tags = cast(List[str], d.pop("tags", UNSET))

        model_schema = cls(
            capabilities=capabilities,
            display_name=display_name,
            id=id,
            media_type=media_type,
            pricing=pricing,
            prompt=prompt,
            provider=provider,
            description=description,
            extra_allowlist=extra_allowlist,
            params=params,
            ref_inputs=ref_inputs,
            tags=tags,
        )

        model_schema.additional_properties = d
        return model_schema

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
