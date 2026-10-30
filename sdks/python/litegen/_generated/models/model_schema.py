from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.capability_media_type import CapabilityMediaType
from ..models.model_task import ModelTask
from ..models.preserves_unmasked import PreservesUnmasked
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.capability_model_pricing import CapabilityModelPricing
    from ..models.model_capability_flags import ModelCapabilityFlags
    from ..models.model_schema_params import ModelSchemaParams
    from ..models.native_orientation import NativeOrientation
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
        prompt (PromptSpec): How a model treats the request's `prompt`. Lengths count characters.
        provider (str):
        deprecated_at (Union[None, Unset, str]): When the vendor deprecated the model (`YYYY-MM-DD`); it still serves it
            (L14).
        description (Union[Unset, str]):
        docs_url (Union[None, Unset, str]): The vendor's own API reference page for the endpoint this model calls —
            the page to re-read when a field mapping is questioned. An absolute
            `https://` URL (the loader refuses anything else); every non-mock 3D
            model carries one.
        extra_allowlist (Union[Unset, List[str]]):
        native_orientation (Union['NativeOrientation', None, Unset]):
        params (Union[Unset, ModelSchemaParams]):
        preserves_unmasked (Union[None, PreservesUnmasked, Unset]):
        ref_inputs (Union['RefInputSpec', None, Unset]):
        replaced_by (Union[None, Unset, str]): The catalog model to move to.
        retires_at (Union[None, Unset, str]): The vendor's announced shutdown date (`YYYY-MM-DD`). From the day
            after it, litegen neither lists nor serves the model.
        tags (Union[Unset, List[str]]):
        task (Union[Unset, ModelTask]): What a catalog model does.

            An operation on an existing asset is a model of its own, submitted to its
            modality's one job endpoint: `tripo/rig` and `meshy/animate` go to
            `POST /v1/models3d/generations` like any generation, and are polled,
            priced, cancelled, re-hosted and billed the same way, because each
            produces a new asset.

            One task per model. `generate` is the default in every modality. It still
            covers image edits driven by `init` and `mask` references, and 3D models
            that infer text, image or multiview input from their references.
    """

    capabilities: "ModelCapabilityFlags"
    display_name: str
    id: str
    media_type: CapabilityMediaType
    pricing: "CapabilityModelPricing"
    prompt: "PromptSpec"
    provider: str
    deprecated_at: Union[None, Unset, str] = UNSET
    description: Union[Unset, str] = UNSET
    docs_url: Union[None, Unset, str] = UNSET
    extra_allowlist: Union[Unset, List[str]] = UNSET
    native_orientation: Union["NativeOrientation", None, Unset] = UNSET
    params: Union[Unset, "ModelSchemaParams"] = UNSET
    preserves_unmasked: Union[None, PreservesUnmasked, Unset] = UNSET
    ref_inputs: Union["RefInputSpec", None, Unset] = UNSET
    replaced_by: Union[None, Unset, str] = UNSET
    retires_at: Union[None, Unset, str] = UNSET
    tags: Union[Unset, List[str]] = UNSET
    task: Union[Unset, ModelTask] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.native_orientation import NativeOrientation
        from ..models.ref_input_spec import RefInputSpec

        capabilities = self.capabilities.to_dict()

        display_name = self.display_name

        id = self.id

        media_type = self.media_type.value

        pricing = self.pricing.to_dict()

        prompt = self.prompt.to_dict()

        provider = self.provider

        deprecated_at: Union[None, Unset, str]
        if isinstance(self.deprecated_at, Unset):
            deprecated_at = UNSET
        else:
            deprecated_at = self.deprecated_at

        description = self.description

        docs_url: Union[None, Unset, str]
        if isinstance(self.docs_url, Unset):
            docs_url = UNSET
        else:
            docs_url = self.docs_url

        extra_allowlist: Union[Unset, List[str]] = UNSET
        if not isinstance(self.extra_allowlist, Unset):
            extra_allowlist = self.extra_allowlist

        native_orientation: Union[Dict[str, Any], None, Unset]
        if isinstance(self.native_orientation, Unset):
            native_orientation = UNSET
        elif isinstance(self.native_orientation, NativeOrientation):
            native_orientation = self.native_orientation.to_dict()
        else:
            native_orientation = self.native_orientation

        params: Union[Unset, Dict[str, Any]] = UNSET
        if not isinstance(self.params, Unset):
            params = self.params.to_dict()

        preserves_unmasked: Union[None, Unset, str]
        if isinstance(self.preserves_unmasked, Unset):
            preserves_unmasked = UNSET
        elif isinstance(self.preserves_unmasked, PreservesUnmasked):
            preserves_unmasked = self.preserves_unmasked.value
        else:
            preserves_unmasked = self.preserves_unmasked

        ref_inputs: Union[Dict[str, Any], None, Unset]
        if isinstance(self.ref_inputs, Unset):
            ref_inputs = UNSET
        elif isinstance(self.ref_inputs, RefInputSpec):
            ref_inputs = self.ref_inputs.to_dict()
        else:
            ref_inputs = self.ref_inputs

        replaced_by: Union[None, Unset, str]
        if isinstance(self.replaced_by, Unset):
            replaced_by = UNSET
        else:
            replaced_by = self.replaced_by

        retires_at: Union[None, Unset, str]
        if isinstance(self.retires_at, Unset):
            retires_at = UNSET
        else:
            retires_at = self.retires_at

        tags: Union[Unset, List[str]] = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        task: Union[Unset, str] = UNSET
        if not isinstance(self.task, Unset):
            task = self.task.value

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
        if deprecated_at is not UNSET:
            field_dict["deprecated_at"] = deprecated_at
        if description is not UNSET:
            field_dict["description"] = description
        if docs_url is not UNSET:
            field_dict["docs_url"] = docs_url
        if extra_allowlist is not UNSET:
            field_dict["extra_allowlist"] = extra_allowlist
        if native_orientation is not UNSET:
            field_dict["native_orientation"] = native_orientation
        if params is not UNSET:
            field_dict["params"] = params
        if preserves_unmasked is not UNSET:
            field_dict["preserves_unmasked"] = preserves_unmasked
        if ref_inputs is not UNSET:
            field_dict["ref_inputs"] = ref_inputs
        if replaced_by is not UNSET:
            field_dict["replaced_by"] = replaced_by
        if retires_at is not UNSET:
            field_dict["retires_at"] = retires_at
        if tags is not UNSET:
            field_dict["tags"] = tags
        if task is not UNSET:
            field_dict["task"] = task

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.capability_model_pricing import CapabilityModelPricing
        from ..models.model_capability_flags import ModelCapabilityFlags
        from ..models.model_schema_params import ModelSchemaParams
        from ..models.native_orientation import NativeOrientation
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

        def _parse_deprecated_at(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        deprecated_at = _parse_deprecated_at(d.pop("deprecated_at", UNSET))

        description = d.pop("description", UNSET)

        def _parse_docs_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        docs_url = _parse_docs_url(d.pop("docs_url", UNSET))

        extra_allowlist = cast(List[str], d.pop("extra_allowlist", UNSET))

        def _parse_native_orientation(
            data: object,
        ) -> Union["NativeOrientation", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                native_orientation_type_1 = NativeOrientation.from_dict(data)

                return native_orientation_type_1
            except:  # noqa: E722
                pass
            return cast(Union["NativeOrientation", None, Unset], data)

        native_orientation = _parse_native_orientation(
            d.pop("native_orientation", UNSET)
        )

        _params = d.pop("params", UNSET)
        params: Union[Unset, ModelSchemaParams]
        if isinstance(_params, Unset):
            params = UNSET
        else:
            params = ModelSchemaParams.from_dict(_params)

        def _parse_preserves_unmasked(
            data: object,
        ) -> Union[None, PreservesUnmasked, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                preserves_unmasked_type_1 = PreservesUnmasked(data)

                return preserves_unmasked_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, PreservesUnmasked, Unset], data)

        preserves_unmasked = _parse_preserves_unmasked(
            d.pop("preserves_unmasked", UNSET)
        )

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

        def _parse_replaced_by(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        replaced_by = _parse_replaced_by(d.pop("replaced_by", UNSET))

        def _parse_retires_at(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        retires_at = _parse_retires_at(d.pop("retires_at", UNSET))

        tags = cast(List[str], d.pop("tags", UNSET))

        _task = d.pop("task", UNSET)
        task: Union[Unset, ModelTask]
        if isinstance(_task, Unset):
            task = UNSET
        else:
            task = ModelTask(_task)

        model_schema = cls(
            capabilities=capabilities,
            display_name=display_name,
            id=id,
            media_type=media_type,
            pricing=pricing,
            prompt=prompt,
            provider=provider,
            deprecated_at=deprecated_at,
            description=description,
            docs_url=docs_url,
            extra_allowlist=extra_allowlist,
            native_orientation=native_orientation,
            params=params,
            preserves_unmasked=preserves_unmasked,
            ref_inputs=ref_inputs,
            replaced_by=replaced_by,
            retires_at=retires_at,
            tags=tags,
            task=task,
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
