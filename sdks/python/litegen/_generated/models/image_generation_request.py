from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.image_background import ImageBackground
from ..models.image_output_format import ImageOutputFormat
from ..models.image_view import ImageView
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.expand import Expand
    from ..models.reference_image import ReferenceImage
    from ..models.region import Region


T = TypeVar("T", bound="ImageGenerationRequest")


@_attrs_define
class ImageGenerationRequest:
    """
    Attributes:
        model (str):
        extra (Union[Unset, Any]):
        metadata (Union[Unset, Any]):
        n (Union[Unset, int]):
        negative_prompt (Union[None, Unset, str]):
        prompt (Union[Unset, str]): Whether a prompt is required is decided per model, by its
            `prompt.required` in `GET /v1/models/{id}` — never by the wire format.
            Omitting it is the same as sending `""`, which an image-to-3D model
            with no prompt field (TRELLIS, Hunyuan3D v2) accepts and a text model
            answers with `prompt_required`.
        reference_images (Union[Unset, List['ReferenceImage']]):
        seed (Union[None, Unset, int]):
        strict (Union[Unset, bool]):
        aspect_ratio (Union[None, Unset, str]):
        background (Union[ImageBackground, None, Unset]):
        expand (Union['Expand', None, Unset]):
        guidance_scale (Union[None, Unset, float]):
        mask_grow_px (Union[None, Unset, int]): Grow the edit area outward by this many pixels: the vendor's own
            setting where it has one (sent explicitly, even as 0), else litegen
            dilates the mask itself. Default 0.
        output_format (Union[ImageOutputFormat, None, Unset]):
        quality (Union[None, Unset, str]):
        region (Union['Region', None, Unset]):
        response_format (Union[Unset, str]):
        size (Union[None, Unset, str]):
        steps (Union[None, Unset, int]):
        strength (Union[None, Unset, float]):
        style (Union[None, Unset, str]):
        views (Union[List[ImageView], None, Unset]): The views a `multiview` model makes, one image each, in this order.
            Omitted means the model's default set.
    """

    model: str
    extra: Union[Unset, Any] = UNSET
    metadata: Union[Unset, Any] = UNSET
    n: Union[Unset, int] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    prompt: Union[Unset, str] = UNSET
    reference_images: Union[Unset, List["ReferenceImage"]] = UNSET
    seed: Union[None, Unset, int] = UNSET
    strict: Union[Unset, bool] = UNSET
    aspect_ratio: Union[None, Unset, str] = UNSET
    background: Union[ImageBackground, None, Unset] = UNSET
    expand: Union["Expand", None, Unset] = UNSET
    guidance_scale: Union[None, Unset, float] = UNSET
    mask_grow_px: Union[None, Unset, int] = UNSET
    output_format: Union[ImageOutputFormat, None, Unset] = UNSET
    quality: Union[None, Unset, str] = UNSET
    region: Union["Region", None, Unset] = UNSET
    response_format: Union[Unset, str] = UNSET
    size: Union[None, Unset, str] = UNSET
    steps: Union[None, Unset, int] = UNSET
    strength: Union[None, Unset, float] = UNSET
    style: Union[None, Unset, str] = UNSET
    views: Union[List[ImageView], None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.expand import Expand
        from ..models.region import Region

        model = self.model

        extra = self.extra

        metadata = self.metadata

        n = self.n

        negative_prompt: Union[None, Unset, str]
        if isinstance(self.negative_prompt, Unset):
            negative_prompt = UNSET
        else:
            negative_prompt = self.negative_prompt

        prompt = self.prompt

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

        background: Union[None, Unset, str]
        if isinstance(self.background, Unset):
            background = UNSET
        elif isinstance(self.background, ImageBackground):
            background = self.background.value
        else:
            background = self.background

        expand: Union[Dict[str, Any], None, Unset]
        if isinstance(self.expand, Unset):
            expand = UNSET
        elif isinstance(self.expand, Expand):
            expand = self.expand.to_dict()
        else:
            expand = self.expand

        guidance_scale: Union[None, Unset, float]
        if isinstance(self.guidance_scale, Unset):
            guidance_scale = UNSET
        else:
            guidance_scale = self.guidance_scale

        mask_grow_px: Union[None, Unset, int]
        if isinstance(self.mask_grow_px, Unset):
            mask_grow_px = UNSET
        else:
            mask_grow_px = self.mask_grow_px

        output_format: Union[None, Unset, str]
        if isinstance(self.output_format, Unset):
            output_format = UNSET
        elif isinstance(self.output_format, ImageOutputFormat):
            output_format = self.output_format.value
        else:
            output_format = self.output_format

        quality: Union[None, Unset, str]
        if isinstance(self.quality, Unset):
            quality = UNSET
        else:
            quality = self.quality

        region: Union[Dict[str, Any], None, Unset]
        if isinstance(self.region, Unset):
            region = UNSET
        elif isinstance(self.region, Region):
            region = self.region.to_dict()
        else:
            region = self.region

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

        views: Union[List[str], None, Unset]
        if isinstance(self.views, Unset):
            views = UNSET
        elif isinstance(self.views, list):
            views = []
            for views_type_0_item_data in self.views:
                views_type_0_item = views_type_0_item_data.value
                views.append(views_type_0_item)

        else:
            views = self.views

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "model": model,
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
        if prompt is not UNSET:
            field_dict["prompt"] = prompt
        if reference_images is not UNSET:
            field_dict["reference_images"] = reference_images
        if seed is not UNSET:
            field_dict["seed"] = seed
        if strict is not UNSET:
            field_dict["strict"] = strict
        if aspect_ratio is not UNSET:
            field_dict["aspect_ratio"] = aspect_ratio
        if background is not UNSET:
            field_dict["background"] = background
        if expand is not UNSET:
            field_dict["expand"] = expand
        if guidance_scale is not UNSET:
            field_dict["guidance_scale"] = guidance_scale
        if mask_grow_px is not UNSET:
            field_dict["mask_grow_px"] = mask_grow_px
        if output_format is not UNSET:
            field_dict["output_format"] = output_format
        if quality is not UNSET:
            field_dict["quality"] = quality
        if region is not UNSET:
            field_dict["region"] = region
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
        if views is not UNSET:
            field_dict["views"] = views

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.expand import Expand
        from ..models.reference_image import ReferenceImage
        from ..models.region import Region

        d = src_dict.copy()
        model = d.pop("model")

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

        prompt = d.pop("prompt", UNSET)

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

        def _parse_background(data: object) -> Union[ImageBackground, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                background_type_1 = ImageBackground(data)

                return background_type_1
            except:  # noqa: E722
                pass
            return cast(Union[ImageBackground, None, Unset], data)

        background = _parse_background(d.pop("background", UNSET))

        def _parse_expand(data: object) -> Union["Expand", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                expand_type_1 = Expand.from_dict(data)

                return expand_type_1
            except:  # noqa: E722
                pass
            return cast(Union["Expand", None, Unset], data)

        expand = _parse_expand(d.pop("expand", UNSET))

        def _parse_guidance_scale(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        guidance_scale = _parse_guidance_scale(d.pop("guidance_scale", UNSET))

        def _parse_mask_grow_px(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        mask_grow_px = _parse_mask_grow_px(d.pop("mask_grow_px", UNSET))

        def _parse_output_format(data: object) -> Union[ImageOutputFormat, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                output_format_type_1 = ImageOutputFormat(data)

                return output_format_type_1
            except:  # noqa: E722
                pass
            return cast(Union[ImageOutputFormat, None, Unset], data)

        output_format = _parse_output_format(d.pop("output_format", UNSET))

        def _parse_quality(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        quality = _parse_quality(d.pop("quality", UNSET))

        def _parse_region(data: object) -> Union["Region", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                region_type_1 = Region.from_dict(data)

                return region_type_1
            except:  # noqa: E722
                pass
            return cast(Union["Region", None, Unset], data)

        region = _parse_region(d.pop("region", UNSET))

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

        def _parse_views(data: object) -> Union[List[ImageView], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                views_type_0 = []
                _views_type_0 = data
                for views_type_0_item_data in _views_type_0:
                    views_type_0_item = ImageView(views_type_0_item_data)

                    views_type_0.append(views_type_0_item)

                return views_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List[ImageView], None, Unset], data)

        views = _parse_views(d.pop("views", UNSET))

        image_generation_request = cls(
            model=model,
            extra=extra,
            metadata=metadata,
            n=n,
            negative_prompt=negative_prompt,
            prompt=prompt,
            reference_images=reference_images,
            seed=seed,
            strict=strict,
            aspect_ratio=aspect_ratio,
            background=background,
            expand=expand,
            guidance_scale=guidance_scale,
            mask_grow_px=mask_grow_px,
            output_format=output_format,
            quality=quality,
            region=region,
            response_format=response_format,
            size=size,
            steps=steps,
            strength=strength,
            style=style,
            views=views,
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
