from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.reference_image import ReferenceImage


T = TypeVar("T", bound="ImageGenerationRequest")


@_attrs_define
class ImageGenerationRequest:
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
        aspect_ratio (Union[None, Unset, str]):
        guidance_scale (Union[None, Unset, float]):
        quality (Union[None, Unset, str]):
        response_format (Union[Unset, str]):
        size (Union[None, Unset, str]):
        steps (Union[None, Unset, int]):
        strength (Union[None, Unset, float]):
        style (Union[None, Unset, str]):
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
    aspect_ratio: Union[None, Unset, str] = UNSET
    guidance_scale: Union[None, Unset, float] = UNSET
    quality: Union[None, Unset, str] = UNSET
    response_format: Union[Unset, str] = UNSET
    size: Union[None, Unset, str] = UNSET
    steps: Union[None, Unset, int] = UNSET
    strength: Union[None, Unset, float] = UNSET
    style: Union[None, Unset, str] = UNSET
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

        aspect_ratio: Union[None, Unset, str]
        if isinstance(self.aspect_ratio, Unset):
            aspect_ratio = UNSET
        else:
            aspect_ratio = self.aspect_ratio

        guidance_scale: Union[None, Unset, float]
        if isinstance(self.guidance_scale, Unset):
            guidance_scale = UNSET
        else:
            guidance_scale = self.guidance_scale

        quality: Union[None, Unset, str]
        if isinstance(self.quality, Unset):
            quality = UNSET
        else:
            quality = self.quality

        response_format = self.response_format

        size: Union[None, Unset, str]
        if isinstance(self.size, Unset):
            size = UNSET
        else:
            size = self.size

        steps: Union[None, Unset, int]
        if isinstance(self.steps, Unset):
            steps = UNSET
        else:
            steps = self.steps

        strength: Union[None, Unset, float]
        if isinstance(self.strength, Unset):
            strength = UNSET
        else:
            strength = self.strength

        style: Union[None, Unset, str]
        if isinstance(self.style, Unset):
            style = UNSET
        else:
            style = self.style

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
        if aspect_ratio is not UNSET:
            field_dict["aspect_ratio"] = aspect_ratio
        if guidance_scale is not UNSET:
            field_dict["guidance_scale"] = guidance_scale
        if quality is not UNSET:
            field_dict["quality"] = quality
        if response_format is not UNSET:
            field_dict["response_format"] = response_format
        if size is not UNSET:
            field_dict["size"] = size
        if steps is not UNSET:
            field_dict["steps"] = steps
        if strength is not UNSET:
            field_dict["strength"] = strength
        if style is not UNSET:
            field_dict["style"] = style

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

        def _parse_aspect_ratio(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        aspect_ratio = _parse_aspect_ratio(d.pop("aspect_ratio", UNSET))

        def _parse_guidance_scale(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        guidance_scale = _parse_guidance_scale(d.pop("guidance_scale", UNSET))

        def _parse_quality(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        quality = _parse_quality(d.pop("quality", UNSET))

        response_format = d.pop("response_format", UNSET)

        def _parse_size(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        size = _parse_size(d.pop("size", UNSET))

        def _parse_steps(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        steps = _parse_steps(d.pop("steps", UNSET))

        def _parse_strength(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        strength = _parse_strength(d.pop("strength", UNSET))

        def _parse_style(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        style = _parse_style(d.pop("style", UNSET))

        image_generation_request = cls(
            model=model,
            prompt=prompt,
            extra=extra,
            metadata=metadata,
            n=n,
            negative_prompt=negative_prompt,
            reference_images=reference_images,
            seed=seed,
            strict=strict,
            aspect_ratio=aspect_ratio,
            guidance_scale=guidance_scale,
            quality=quality,
            response_format=response_format,
            size=size,
            steps=steps,
            strength=strength,
            style=style,
        )

        image_generation_request.additional_properties = d
        return image_generation_request

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
