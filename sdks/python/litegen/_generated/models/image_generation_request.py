from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.image_background import ImageBackground
from ..models.image_output_format import ImageOutputFormat
from ..models.image_view import ImageView
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.base_generation_request_metadata_type_0 import (
        BaseGenerationRequestMetadataType0,
    )
    from ..models.expand import Expand
    from ..models.layout_item import LayoutItem
    from ..models.reference_image import ReferenceImage
    from ..models.region import Region


T = TypeVar("T", bound="ImageGenerationRequest")


@_attrs_define
class ImageGenerationRequest:
    """
    Attributes:
        model (str):
        extra (Union[Unset, Any]):
        metadata (Union['BaseGenerationRequestMetadataType0', None, Unset]): Your own labels for the job (PixelGenie
            L17): a map of string values,
            keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. Kept on
            the generation, echoed on every answer and webhook, and filters
            `GET /v1/logs` and `GET /v1/stats` as `metadata.<key>=<value>`.
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
        directions (Union[None, Unset, int]): A character in 4 or 8 facings, one labelled image each (PixelGenie L16).
        expand (Union['Expand', None, Unset]):
        feather_px (Union[None, Unset, int]): With `preserve_unmasked`: blend from the init at the edit area's edge
            to the vendor's pixels this many pixels inside it (0, a hard edge, by
            default; at most 256).
        guidance_scale (Union[None, Unset, float]):
        layout (Union[List['LayoutItem'], None, Unset]): Boxes for a text-to-image canvas (3D doc §4): objects and words
            placed by fractions of the canvas.
        mask_grow_px (Union[None, Unset, int]): Grow the edit area outward by this many pixels: the vendor's own
            setting where it has one (sent explicitly, even as 0), else litegen
            dilates the mask itself. Default 0.
        mask_prompt (Union[None, Unset, str]): The area an edit replaces, named in text rather than drawn as a mask
            (3D doc §4).
        output_format (Union[ImageOutputFormat, None, Unset]):
        palette (Union[List[str], None, Unset]): Colours as `#rrggbb` (PixelGenie L16): every pixel one of them on a
            model whose palette is forced, a hint elsewhere.
        preserve_unmasked (Union[None, Unset, bool]): Paste the init image's pixels back outside the edit area (3D doc
            §4),
            so they come back byte-identical from any vendor. Needs a mask or a
            region, a lossless container and an output the init's size.
        quality (Union[None, Unset, str]):
        region (Union['Region', None, Unset]):
        resolution (Union[None, Unset, str]): The output tier, on a model that has tiers: one of its
            `params.resolution` values, on one scale for every vendor (`0.5K`,
            `0.75K`, `1K`, `1.5K`, `2K`, `4K`, about that many thousand pixels a
            side; FLUX 3 Image adds `match_input`, an edit at its reference's
            size). An exact `size` wins where a model takes both.
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
    metadata: Union["BaseGenerationRequestMetadataType0", None, Unset] = UNSET
    n: Union[Unset, int] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    prompt: Union[Unset, str] = UNSET
    reference_images: Union[Unset, List["ReferenceImage"]] = UNSET
    seed: Union[None, Unset, int] = UNSET
    strict: Union[Unset, bool] = UNSET
    aspect_ratio: Union[None, Unset, str] = UNSET
    background: Union[ImageBackground, None, Unset] = UNSET
    directions: Union[None, Unset, int] = UNSET
    expand: Union["Expand", None, Unset] = UNSET
    feather_px: Union[None, Unset, int] = UNSET
    guidance_scale: Union[None, Unset, float] = UNSET
    layout: Union[List["LayoutItem"], None, Unset] = UNSET
    mask_grow_px: Union[None, Unset, int] = UNSET
    mask_prompt: Union[None, Unset, str] = UNSET
    output_format: Union[ImageOutputFormat, None, Unset] = UNSET
    palette: Union[List[str], None, Unset] = UNSET
    preserve_unmasked: Union[None, Unset, bool] = UNSET
    quality: Union[None, Unset, str] = UNSET
    region: Union["Region", None, Unset] = UNSET
    resolution: Union[None, Unset, str] = UNSET
    response_format: Union[Unset, str] = UNSET
    size: Union[None, Unset, str] = UNSET
    steps: Union[None, Unset, int] = UNSET
    strength: Union[None, Unset, float] = UNSET
    style: Union[None, Unset, str] = UNSET
    views: Union[List[ImageView], None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.expand import Expand
        from ..models.region import Region

        model = self.model

        extra = self.extra

        metadata: Union[Dict[str, Any], None, Unset]
        if isinstance(self.metadata, Unset):
            metadata = UNSET
        elif isinstance(self.metadata, BaseGenerationRequestMetadataType0):
            metadata = self.metadata.to_dict()
        else:
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

        directions: Union[None, Unset, int]
        if isinstance(self.directions, Unset):
            directions = UNSET
        else:
            directions = self.directions

        expand: Union[Dict[str, Any], None, Unset]
        if isinstance(self.expand, Unset):
            expand = UNSET
        elif isinstance(self.expand, Expand):
            expand = self.expand.to_dict()
        else:
            expand = self.expand

        feather_px: Union[None, Unset, int]
        if isinstance(self.feather_px, Unset):
            feather_px = UNSET
        else:
            feather_px = self.feather_px

        guidance_scale: Union[None, Unset, float]
        if isinstance(self.guidance_scale, Unset):
            guidance_scale = UNSET
        else:
            guidance_scale = self.guidance_scale

        layout: Union[List[Dict[str, Any]], None, Unset]
        if isinstance(self.layout, Unset):
            layout = UNSET
        elif isinstance(self.layout, list):
            layout = []
            for layout_type_0_item_data in self.layout:
                layout_type_0_item = layout_type_0_item_data.to_dict()
                layout.append(layout_type_0_item)

        else:
            layout = self.layout

        mask_grow_px: Union[None, Unset, int]
        if isinstance(self.mask_grow_px, Unset):
            mask_grow_px = UNSET
        else:
            mask_grow_px = self.mask_grow_px

        mask_prompt: Union[None, Unset, str]
        if isinstance(self.mask_prompt, Unset):
            mask_prompt = UNSET
        else:
            mask_prompt = self.mask_prompt

        output_format: Union[None, Unset, str]
        if isinstance(self.output_format, Unset):
            output_format = UNSET
        elif isinstance(self.output_format, ImageOutputFormat):
            output_format = self.output_format.value
        else:
            output_format = self.output_format

        palette: Union[List[str], None, Unset]
        if isinstance(self.palette, Unset):
            palette = UNSET
        elif isinstance(self.palette, list):
            palette = self.palette

        else:
            palette = self.palette

        preserve_unmasked: Union[None, Unset, bool]
        if isinstance(self.preserve_unmasked, Unset):
            preserve_unmasked = UNSET
        else:
            preserve_unmasked = self.preserve_unmasked

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

        resolution: Union[None, Unset, str]
        if isinstance(self.resolution, Unset):
            resolution = UNSET
        else:
            resolution = self.resolution

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
        if directions is not UNSET:
            field_dict["directions"] = directions
        if expand is not UNSET:
            field_dict["expand"] = expand
        if feather_px is not UNSET:
            field_dict["feather_px"] = feather_px
        if guidance_scale is not UNSET:
            field_dict["guidance_scale"] = guidance_scale
        if layout is not UNSET:
            field_dict["layout"] = layout
        if mask_grow_px is not UNSET:
            field_dict["mask_grow_px"] = mask_grow_px
        if mask_prompt is not UNSET:
            field_dict["mask_prompt"] = mask_prompt
        if output_format is not UNSET:
            field_dict["output_format"] = output_format
        if palette is not UNSET:
            field_dict["palette"] = palette
        if preserve_unmasked is not UNSET:
            field_dict["preserve_unmasked"] = preserve_unmasked
        if quality is not UNSET:
            field_dict["quality"] = quality
        if region is not UNSET:
            field_dict["region"] = region
        if resolution is not UNSET:
            field_dict["resolution"] = resolution
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
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.expand import Expand
        from ..models.layout_item import LayoutItem
        from ..models.reference_image import ReferenceImage
        from ..models.region import Region

        d = src_dict.copy()
        model = d.pop("model")

        extra = d.pop("extra", UNSET)

        def _parse_metadata(
            data: object,
        ) -> Union["BaseGenerationRequestMetadataType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                metadata_type_0 = BaseGenerationRequestMetadataType0.from_dict(data)

                return metadata_type_0
            except:  # noqa: E722
                pass
            return cast(Union["BaseGenerationRequestMetadataType0", None, Unset], data)

        metadata = _parse_metadata(d.pop("metadata", UNSET))

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

        def _parse_directions(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        directions = _parse_directions(d.pop("directions", UNSET))

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

        def _parse_feather_px(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        feather_px = _parse_feather_px(d.pop("feather_px", UNSET))

        def _parse_guidance_scale(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        guidance_scale = _parse_guidance_scale(d.pop("guidance_scale", UNSET))

        def _parse_layout(data: object) -> Union[List["LayoutItem"], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                layout_type_0 = []
                _layout_type_0 = data
                for layout_type_0_item_data in _layout_type_0:
                    layout_type_0_item = LayoutItem.from_dict(layout_type_0_item_data)

                    layout_type_0.append(layout_type_0_item)

                return layout_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List["LayoutItem"], None, Unset], data)

        layout = _parse_layout(d.pop("layout", UNSET))

        def _parse_mask_grow_px(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        mask_grow_px = _parse_mask_grow_px(d.pop("mask_grow_px", UNSET))

        def _parse_mask_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        mask_prompt = _parse_mask_prompt(d.pop("mask_prompt", UNSET))

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

        def _parse_palette(data: object) -> Union[List[str], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                palette_type_0 = cast(List[str], data)

                return palette_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List[str], None, Unset], data)

        palette = _parse_palette(d.pop("palette", UNSET))

        def _parse_preserve_unmasked(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        preserve_unmasked = _parse_preserve_unmasked(d.pop("preserve_unmasked", UNSET))

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

        def _parse_resolution(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        resolution = _parse_resolution(d.pop("resolution", UNSET))

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
            directions=directions,
            expand=expand,
            feather_px=feather_px,
            guidance_scale=guidance_scale,
            layout=layout,
            mask_grow_px=mask_grow_px,
            mask_prompt=mask_prompt,
            output_format=output_format,
            palette=palette,
            preserve_unmasked=preserve_unmasked,
            quality=quality,
            region=region,
            resolution=resolution,
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
