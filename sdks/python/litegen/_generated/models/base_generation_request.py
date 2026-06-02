from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.reference_image import ReferenceImage


T = TypeVar("T", bound="BaseGenerationRequest")


@_attrs_define
class BaseGenerationRequest:
    """
    Attributes:
        model (str):
        prompt (str):
        extra (Union[Unset, Any]):
        metadata (Union[Unset, Any]):
        n (Union[Unset, int]):
        negative_prompt (Union[None, Unset, str]):
        reference_images (Union[Unset, List['ReferenceImage']]):
        seed (Union[None, Unset, int]):
        strict (Union[Unset, bool]):
    """

    model: str
    prompt: str
    extra: Union[Unset, Any] = UNSET
    metadata: Union[Unset, Any] = UNSET
    n: Union[Unset, int] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    reference_images: Union[Unset, List["ReferenceImage"]] = UNSET
    seed: Union[None, Unset, int] = UNSET
    strict: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        model = self.model

        prompt = self.prompt

        extra = self.extra

        metadata = self.metadata

        n = self.n

        negative_prompt: Union[None, Unset, str]
        if isinstance(self.negative_prompt, Unset):
            negative_prompt = UNSET
        else:
            negative_prompt = self.negative_prompt

        reference_images: Union[Unset, List[Dict[str, Any]]] = UNSET
        if not isinstance(self.reference_images, Unset):
            reference_images = []
            for reference_images_item_data in self.reference_images:
                reference_images_item = reference_images_item_data.to_dict()
                reference_images.append(reference_images_item)

        seed: Union[None, Unset, int]
        if isinstance(self.seed, Unset):
            seed = UNSET
        else:
            seed = self.seed

        strict = self.strict

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "model": model,
                "prompt": prompt,
            }
        )
        if extra is not UNSET:
            field_dict["extra"] = extra
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if n is not UNSET:
            field_dict["n"] = n
        if negative_prompt is not UNSET:
            field_dict["negative_prompt"] = negative_prompt
        if reference_images is not UNSET:
            field_dict["reference_images"] = reference_images
        if seed is not UNSET:
            field_dict["seed"] = seed
        if strict is not UNSET:
            field_dict["strict"] = strict

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.reference_image import ReferenceImage

        d = src_dict.copy()
        model = d.pop("model")

        prompt = d.pop("prompt")

        extra = d.pop("extra", UNSET)

        metadata = d.pop("metadata", UNSET)

        n = d.pop("n", UNSET)

        def _parse_negative_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        negative_prompt = _parse_negative_prompt(d.pop("negative_prompt", UNSET))

        reference_images = []
        _reference_images = d.pop("reference_images", UNSET)
        for reference_images_item_data in _reference_images or []:
            reference_images_item = ReferenceImage.from_dict(reference_images_item_data)

            reference_images.append(reference_images_item)

        def _parse_seed(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        seed = _parse_seed(d.pop("seed", UNSET))

        strict = d.pop("strict", UNSET)

        base_generation_request = cls(
            model=model,
            prompt=prompt,
            extra=extra,
            metadata=metadata,
            n=n,
            negative_prompt=negative_prompt,
            reference_images=reference_images,
            seed=seed,
            strict=strict,
        )

        base_generation_request.additional_properties = d
        return base_generation_request

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
