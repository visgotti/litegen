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

from ..models.extend_output import ExtendOutput
from ..models.keyframe_timing import KeyframeTiming
from ..models.layout_kind import LayoutKind
from ..models.mask_convention import MaskConvention
from ..models.model_task import ModelTask
from ..models.preserves_unmasked import PreservesUnmasked
from ..models.region_input import RegionInput
from ..models.segmentation_prompt_type import SegmentationPromptType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.pixel_size import PixelSize


T = TypeVar("T", bound="ModelCapabilities")


@_attrs_define
class ModelCapabilities:
    """
    Attributes:
        supports_image_to_image (bool):
        supports_text_to_image (bool):
        task (ModelTask): What a catalog model does.

            An operation on an existing asset is a model of its own, submitted to its
            modality's one job endpoint: `tripo/rig` and `meshy/animate` go to
            `POST /v1/models3d/generations` like any generation, and are polled,
            priced, cancelled, re-hosted and billed the same way, because each
            produces a new asset.

            One task per model. `generate` is the default in every modality. It still
            covers image edits driven by `init` and `mask` references, and 3D models
            that infer text, image or multiview input from their references.
        directions (Union[Unset, List[int]]): The `directions` counts it takes; empty when none.
        duration_values (Union[Unset, List[float]]): The only `duration_seconds` values the model takes; empty when it
            takes its range.
        extend_output (Union[ExtendOutput, None, Unset]):
        keyframe_timing (Union[KeyframeTiming, None, Unset]):
        layout_kinds (Union[Unset, List[LayoutKind]]): What its layout boxes may hold; empty when it takes no layout.
        lossy_only (Union[Unset, bool]): Returns only lossy containers (no PNG).
        mask_convention (Union[MaskConvention, None, Unset]):
        max_duration_seconds (Union[None, Unset, float]): The longest clip the model makes, from its `duration_seconds`
            param.
        max_images (Union[Unset, int]):
        max_keyframes (Union[None, Unset, int]): The most `keyframe` references one request may carry.
        max_polycount (Union[None, Unset, int]): Upper bound of the model's `target_polycount` param, when it takes one.
        max_reference_images (Union[Unset, int]): The most reference images one request may carry. Kept under its old
            name too: `max_images` is this same number (L14).
        native_pixel_art (Union[Unset, bool]): Draws native pixel art (PixelGenie L16).
        output_formats (Union[Unset, List[str]]): Every mesh container a caller may request, including those litegen
            derives locally — the same list, in the same order, as
            `GET /v1/models/{id}` serves under `params.output_formats.enum_values`.
            Empty for image and video models.
        pixel_size (Union['PixelSize', None, Unset]):
        preserves_unmasked (Union[None, PreservesUnmasked, Unset]):
        region_input (Union[None, RegionInput, Unset]):
        segmentation_prompts (Union[Unset, List[SegmentationPromptType]]): The prompt types it segments by: `point`,
            `box`, `text`. Empty when it
            does not segment.
        supported_sizes (Union[Unset, List[str]]):
        supports_animation (Union[Unset, bool]): Takes `animations`: one clip per entry, whatever the task (3D doc
            §3C) — a rig or generate model that returns clips in the same call
            as well as an `animate` one.
        supports_audio_toggle (Union[Unset, bool]): Takes `audio: false` (or `true`) to leave audio out (or in).
        supports_background_removal (Union[Unset, bool]): Takes one `init` image and returns it as an RGBA PNG with its
            background removed (PixelGenie L8).
        supports_expand (Union[Unset, bool]): Takes `expand` (3D doc §4).
        supports_first_frame (Union[Unset, bool]):
        supports_fixed_camera (Union[Unset, bool]): Takes `camera: fixed` (PixelGenie L10).
        supports_forced_palette (Union[Unset, bool]): Every pixel of its output is one of the palette's colours.
        supports_image_to_3d (Union[Unset, bool]): Accepts one `init` reference image (image-to-3D).
        supports_image_to_video (Union[Unset, bool]):
        supports_inpainting (Union[Unset, bool]):
        supports_joint_naming (Union[Unset, bool]): Takes `joint_naming` (3D doc §3B).
        supports_keep_uvs (Union[Unset, bool]): Takes `keep_uvs`.
        supports_keyframes (Union[Unset, bool]): Takes `keyframe` references, each pinned at its `time_s` (3D doc §5).
        supports_last_frame (Union[Unset, bool]):
        supports_layout (Union[Unset, bool]): Takes `layout` (3D doc §4).
        supports_loop (Union[Unset, bool]): Takes `loop: true`.
        supports_mask_grow (Union[Unset, bool]): Takes `mask_grow_px`: litegen widens the mask or region of any model
            taking one, or asks a vendor that grows its own (3D doc §4).
        supports_mask_prompt (Union[Unset, bool]): Takes `mask_prompt` (3D doc §4).
        supports_multiview_to_3d (Union[Unset, bool]): Accepts several `view-*` reference images (multiview-to-3D).
        supports_palette (Union[Unset, bool]): Takes `palette`, as a hint or forced.
        supports_pbr (Union[Unset, bool]): Takes the `pbr` param.
        supports_placement (Union[Unset, bool]): Takes `ground`, `center` and `height_m`, which litegen applies to every
            mesh it delivers: true on every 3D model.
        supports_preserve_unmasked (Union[Unset, bool]): Takes `preserve_unmasked`, which litegen applies to any model
            taking a
            mask or a region (3D doc §4).
        supports_region_edit (Union[Unset, bool]): Takes `region` (3D doc §4).
        supports_rig (Union[Unset, bool]): Takes the `rig` param.
        supports_segmentation (Union[Unset, bool]): Serves `POST /v1/images/segmentations` (PixelGenie L7).
        supports_skeleton (Union[Unset, bool]): Takes `skeleton`; `params.skeleton.enum_values` lists what it rigs
            (3D doc §3B).
        supports_source_model (Union[Unset, bool]): Takes `source_model`: an operation on an existing mesh.
        supports_source_video (Union[Unset, bool]): Works on a `source_video` (3D doc §5).
        supports_text_to_3d (Union[Unset, bool]): Accepts a prompt with no reference images (text-to-3D).
        supports_text_to_video (Union[Unset, bool]):
        supports_texture (Union[Unset, bool]): Takes the `texture` param.
        supports_time_range (Union[Unset, bool]): Takes `time_range`.
        supports_transparent_background (Union[Unset, bool]): Takes `background: transparent` (PixelGenie L9).
        texture_resolutions (Union[Unset, List[int]]): The map sizes a `texture` model paints at (`texture_resolution`);
            empty when it takes none.
    """

    supports_image_to_image: bool
    supports_text_to_image: bool
    task: ModelTask
    directions: Union[Unset, List[int]] = UNSET
    duration_values: Union[Unset, List[float]] = UNSET
    extend_output: Union[ExtendOutput, None, Unset] = UNSET
    keyframe_timing: Union[KeyframeTiming, None, Unset] = UNSET
    layout_kinds: Union[Unset, List[LayoutKind]] = UNSET
    lossy_only: Union[Unset, bool] = UNSET
    mask_convention: Union[MaskConvention, None, Unset] = UNSET
    max_duration_seconds: Union[None, Unset, float] = UNSET
    max_images: Union[Unset, int] = UNSET
    max_keyframes: Union[None, Unset, int] = UNSET
    max_polycount: Union[None, Unset, int] = UNSET
    max_reference_images: Union[Unset, int] = UNSET
    native_pixel_art: Union[Unset, bool] = UNSET
    output_formats: Union[Unset, List[str]] = UNSET
    pixel_size: Union["PixelSize", None, Unset] = UNSET
    preserves_unmasked: Union[None, PreservesUnmasked, Unset] = UNSET
    region_input: Union[None, RegionInput, Unset] = UNSET
    segmentation_prompts: Union[Unset, List[SegmentationPromptType]] = UNSET
    supported_sizes: Union[Unset, List[str]] = UNSET
    supports_animation: Union[Unset, bool] = UNSET
    supports_audio_toggle: Union[Unset, bool] = UNSET
    supports_background_removal: Union[Unset, bool] = UNSET
    supports_expand: Union[Unset, bool] = UNSET
    supports_first_frame: Union[Unset, bool] = UNSET
    supports_fixed_camera: Union[Unset, bool] = UNSET
    supports_forced_palette: Union[Unset, bool] = UNSET
    supports_image_to_3d: Union[Unset, bool] = UNSET
    supports_image_to_video: Union[Unset, bool] = UNSET
    supports_inpainting: Union[Unset, bool] = UNSET
    supports_joint_naming: Union[Unset, bool] = UNSET
    supports_keep_uvs: Union[Unset, bool] = UNSET
    supports_keyframes: Union[Unset, bool] = UNSET
    supports_last_frame: Union[Unset, bool] = UNSET
    supports_layout: Union[Unset, bool] = UNSET
    supports_loop: Union[Unset, bool] = UNSET
    supports_mask_grow: Union[Unset, bool] = UNSET
    supports_mask_prompt: Union[Unset, bool] = UNSET
    supports_multiview_to_3d: Union[Unset, bool] = UNSET
    supports_palette: Union[Unset, bool] = UNSET
    supports_pbr: Union[Unset, bool] = UNSET
    supports_placement: Union[Unset, bool] = UNSET
    supports_preserve_unmasked: Union[Unset, bool] = UNSET
    supports_region_edit: Union[Unset, bool] = UNSET
    supports_rig: Union[Unset, bool] = UNSET
    supports_segmentation: Union[Unset, bool] = UNSET
    supports_skeleton: Union[Unset, bool] = UNSET
    supports_source_model: Union[Unset, bool] = UNSET
    supports_source_video: Union[Unset, bool] = UNSET
    supports_text_to_3d: Union[Unset, bool] = UNSET
    supports_text_to_video: Union[Unset, bool] = UNSET
    supports_texture: Union[Unset, bool] = UNSET
    supports_time_range: Union[Unset, bool] = UNSET
    supports_transparent_background: Union[Unset, bool] = UNSET
    texture_resolutions: Union[Unset, List[int]] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.pixel_size import PixelSize

        supports_image_to_image = self.supports_image_to_image

        supports_text_to_image = self.supports_text_to_image

        task = self.task.value

        directions: Union[Unset, List[int]] = UNSET
        if not isinstance(self.directions, Unset):
            directions = self.directions

        duration_values: Union[Unset, List[float]] = UNSET
        if not isinstance(self.duration_values, Unset):
            duration_values = self.duration_values

        extend_output: Union[None, Unset, str]
        if isinstance(self.extend_output, Unset):
            extend_output = UNSET
        elif isinstance(self.extend_output, ExtendOutput):
            extend_output = self.extend_output.value
        else:
            extend_output = self.extend_output

        keyframe_timing: Union[None, Unset, str]
        if isinstance(self.keyframe_timing, Unset):
            keyframe_timing = UNSET
        elif isinstance(self.keyframe_timing, KeyframeTiming):
            keyframe_timing = self.keyframe_timing.value
        else:
            keyframe_timing = self.keyframe_timing

        layout_kinds: Union[Unset, List[str]] = UNSET
        if not isinstance(self.layout_kinds, Unset):
            layout_kinds = []
            for layout_kinds_item_data in self.layout_kinds:
                layout_kinds_item = layout_kinds_item_data.value
                layout_kinds.append(layout_kinds_item)

        lossy_only = self.lossy_only

        mask_convention: Union[None, Unset, str]
        if isinstance(self.mask_convention, Unset):
            mask_convention = UNSET
        elif isinstance(self.mask_convention, MaskConvention):
            mask_convention = self.mask_convention.value
        else:
            mask_convention = self.mask_convention

        max_duration_seconds: Union[None, Unset, float]
        if isinstance(self.max_duration_seconds, Unset):
            max_duration_seconds = UNSET
        else:
            max_duration_seconds = self.max_duration_seconds

        max_images = self.max_images

        max_keyframes: Union[None, Unset, int]
        if isinstance(self.max_keyframes, Unset):
            max_keyframes = UNSET
        else:
            max_keyframes = self.max_keyframes

        max_polycount: Union[None, Unset, int]
        if isinstance(self.max_polycount, Unset):
            max_polycount = UNSET
        else:
            max_polycount = self.max_polycount

        max_reference_images = self.max_reference_images

        native_pixel_art = self.native_pixel_art

        output_formats: Union[Unset, List[str]] = UNSET
        if not isinstance(self.output_formats, Unset):
            output_formats = self.output_formats

        pixel_size: Union[Dict[str, Any], None, Unset]
        if isinstance(self.pixel_size, Unset):
            pixel_size = UNSET
        elif isinstance(self.pixel_size, PixelSize):
            pixel_size = self.pixel_size.to_dict()
        else:
            pixel_size = self.pixel_size

        preserves_unmasked: Union[None, Unset, str]
        if isinstance(self.preserves_unmasked, Unset):
            preserves_unmasked = UNSET
        elif isinstance(self.preserves_unmasked, PreservesUnmasked):
            preserves_unmasked = self.preserves_unmasked.value
        else:
            preserves_unmasked = self.preserves_unmasked

        region_input: Union[None, Unset, str]
        if isinstance(self.region_input, Unset):
            region_input = UNSET
        elif isinstance(self.region_input, RegionInput):
            region_input = self.region_input.value
        else:
            region_input = self.region_input

        segmentation_prompts: Union[Unset, List[str]] = UNSET
        if not isinstance(self.segmentation_prompts, Unset):
            segmentation_prompts = []
            for segmentation_prompts_item_data in self.segmentation_prompts:
                segmentation_prompts_item = segmentation_prompts_item_data.value
                segmentation_prompts.append(segmentation_prompts_item)

        supported_sizes: Union[Unset, List[str]] = UNSET
        if not isinstance(self.supported_sizes, Unset):
            supported_sizes = self.supported_sizes

        supports_animation = self.supports_animation

        supports_audio_toggle = self.supports_audio_toggle

        supports_background_removal = self.supports_background_removal

        supports_expand = self.supports_expand

        supports_first_frame = self.supports_first_frame

        supports_fixed_camera = self.supports_fixed_camera

        supports_forced_palette = self.supports_forced_palette

        supports_image_to_3d = self.supports_image_to_3d

        supports_image_to_video = self.supports_image_to_video

        supports_inpainting = self.supports_inpainting

        supports_joint_naming = self.supports_joint_naming

        supports_keep_uvs = self.supports_keep_uvs

        supports_keyframes = self.supports_keyframes

        supports_last_frame = self.supports_last_frame

        supports_layout = self.supports_layout

        supports_loop = self.supports_loop

        supports_mask_grow = self.supports_mask_grow

        supports_mask_prompt = self.supports_mask_prompt

        supports_multiview_to_3d = self.supports_multiview_to_3d

        supports_palette = self.supports_palette

        supports_pbr = self.supports_pbr

        supports_placement = self.supports_placement

        supports_preserve_unmasked = self.supports_preserve_unmasked

        supports_region_edit = self.supports_region_edit

        supports_rig = self.supports_rig

        supports_segmentation = self.supports_segmentation

        supports_skeleton = self.supports_skeleton

        supports_source_model = self.supports_source_model

        supports_source_video = self.supports_source_video

        supports_text_to_3d = self.supports_text_to_3d

        supports_text_to_video = self.supports_text_to_video

        supports_texture = self.supports_texture

        supports_time_range = self.supports_time_range

        supports_transparent_background = self.supports_transparent_background

        texture_resolutions: Union[Unset, List[int]] = UNSET
        if not isinstance(self.texture_resolutions, Unset):
            texture_resolutions = self.texture_resolutions

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "supports_image_to_image": supports_image_to_image,
                "supports_text_to_image": supports_text_to_image,
                "task": task,
            }
        )
        if directions is not UNSET:
            field_dict["directions"] = directions
        if duration_values is not UNSET:
            field_dict["duration_values"] = duration_values
        if extend_output is not UNSET:
            field_dict["extend_output"] = extend_output
        if keyframe_timing is not UNSET:
            field_dict["keyframe_timing"] = keyframe_timing
        if layout_kinds is not UNSET:
            field_dict["layout_kinds"] = layout_kinds
        if lossy_only is not UNSET:
            field_dict["lossy_only"] = lossy_only
        if mask_convention is not UNSET:
            field_dict["mask_convention"] = mask_convention
        if max_duration_seconds is not UNSET:
            field_dict["max_duration_seconds"] = max_duration_seconds
        if max_images is not UNSET:
            field_dict["max_images"] = max_images
        if max_keyframes is not UNSET:
            field_dict["max_keyframes"] = max_keyframes
        if max_polycount is not UNSET:
            field_dict["max_polycount"] = max_polycount
        if max_reference_images is not UNSET:
            field_dict["max_reference_images"] = max_reference_images
        if native_pixel_art is not UNSET:
            field_dict["native_pixel_art"] = native_pixel_art
        if output_formats is not UNSET:
            field_dict["output_formats"] = output_formats
        if pixel_size is not UNSET:
            field_dict["pixel_size"] = pixel_size
        if preserves_unmasked is not UNSET:
            field_dict["preserves_unmasked"] = preserves_unmasked
        if region_input is not UNSET:
            field_dict["region_input"] = region_input
        if segmentation_prompts is not UNSET:
            field_dict["segmentation_prompts"] = segmentation_prompts
        if supported_sizes is not UNSET:
            field_dict["supported_sizes"] = supported_sizes
        if supports_animation is not UNSET:
            field_dict["supports_animation"] = supports_animation
        if supports_audio_toggle is not UNSET:
            field_dict["supports_audio_toggle"] = supports_audio_toggle
        if supports_background_removal is not UNSET:
            field_dict["supports_background_removal"] = supports_background_removal
        if supports_expand is not UNSET:
            field_dict["supports_expand"] = supports_expand
        if supports_first_frame is not UNSET:
            field_dict["supports_first_frame"] = supports_first_frame
        if supports_fixed_camera is not UNSET:
            field_dict["supports_fixed_camera"] = supports_fixed_camera
        if supports_forced_palette is not UNSET:
            field_dict["supports_forced_palette"] = supports_forced_palette
        if supports_image_to_3d is not UNSET:
            field_dict["supports_image_to_3d"] = supports_image_to_3d
        if supports_image_to_video is not UNSET:
            field_dict["supports_image_to_video"] = supports_image_to_video
        if supports_inpainting is not UNSET:
            field_dict["supports_inpainting"] = supports_inpainting
        if supports_joint_naming is not UNSET:
            field_dict["supports_joint_naming"] = supports_joint_naming
        if supports_keep_uvs is not UNSET:
            field_dict["supports_keep_uvs"] = supports_keep_uvs
        if supports_keyframes is not UNSET:
            field_dict["supports_keyframes"] = supports_keyframes
        if supports_last_frame is not UNSET:
            field_dict["supports_last_frame"] = supports_last_frame
        if supports_layout is not UNSET:
            field_dict["supports_layout"] = supports_layout
        if supports_loop is not UNSET:
            field_dict["supports_loop"] = supports_loop
        if supports_mask_grow is not UNSET:
            field_dict["supports_mask_grow"] = supports_mask_grow
        if supports_mask_prompt is not UNSET:
            field_dict["supports_mask_prompt"] = supports_mask_prompt
        if supports_multiview_to_3d is not UNSET:
            field_dict["supports_multiview_to_3d"] = supports_multiview_to_3d
        if supports_palette is not UNSET:
            field_dict["supports_palette"] = supports_palette
        if supports_pbr is not UNSET:
            field_dict["supports_pbr"] = supports_pbr
        if supports_placement is not UNSET:
            field_dict["supports_placement"] = supports_placement
        if supports_preserve_unmasked is not UNSET:
            field_dict["supports_preserve_unmasked"] = supports_preserve_unmasked
        if supports_region_edit is not UNSET:
            field_dict["supports_region_edit"] = supports_region_edit
        if supports_rig is not UNSET:
            field_dict["supports_rig"] = supports_rig
        if supports_segmentation is not UNSET:
            field_dict["supports_segmentation"] = supports_segmentation
        if supports_skeleton is not UNSET:
            field_dict["supports_skeleton"] = supports_skeleton
        if supports_source_model is not UNSET:
            field_dict["supports_source_model"] = supports_source_model
        if supports_source_video is not UNSET:
            field_dict["supports_source_video"] = supports_source_video
        if supports_text_to_3d is not UNSET:
            field_dict["supports_text_to_3d"] = supports_text_to_3d
        if supports_text_to_video is not UNSET:
            field_dict["supports_text_to_video"] = supports_text_to_video
        if supports_texture is not UNSET:
            field_dict["supports_texture"] = supports_texture
        if supports_time_range is not UNSET:
            field_dict["supports_time_range"] = supports_time_range
        if supports_transparent_background is not UNSET:
            field_dict["supports_transparent_background"] = (
                supports_transparent_background
            )
        if texture_resolutions is not UNSET:
            field_dict["texture_resolutions"] = texture_resolutions

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.pixel_size import PixelSize

        d = src_dict.copy()
        supports_image_to_image = d.pop("supports_image_to_image")

        supports_text_to_image = d.pop("supports_text_to_image")

        task = ModelTask(d.pop("task"))

        directions = cast(List[int], d.pop("directions", UNSET))

        duration_values = cast(List[float], d.pop("duration_values", UNSET))

        def _parse_extend_output(data: object) -> Union[ExtendOutput, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                extend_output_type_1 = ExtendOutput(data)

                return extend_output_type_1
            except:  # noqa: E722
                pass
            return cast(Union[ExtendOutput, None, Unset], data)

        extend_output = _parse_extend_output(d.pop("extend_output", UNSET))

        def _parse_keyframe_timing(data: object) -> Union[KeyframeTiming, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                keyframe_timing_type_1 = KeyframeTiming(data)

                return keyframe_timing_type_1
            except:  # noqa: E722
                pass
            return cast(Union[KeyframeTiming, None, Unset], data)

        keyframe_timing = _parse_keyframe_timing(d.pop("keyframe_timing", UNSET))

        layout_kinds = []
        _layout_kinds = d.pop("layout_kinds", UNSET)
        for layout_kinds_item_data in _layout_kinds or []:
            layout_kinds_item = LayoutKind(layout_kinds_item_data)

            layout_kinds.append(layout_kinds_item)

        lossy_only = d.pop("lossy_only", UNSET)

        def _parse_mask_convention(data: object) -> Union[MaskConvention, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                mask_convention_type_1 = MaskConvention(data)

                return mask_convention_type_1
            except:  # noqa: E722
                pass
            return cast(Union[MaskConvention, None, Unset], data)

        mask_convention = _parse_mask_convention(d.pop("mask_convention", UNSET))

        def _parse_max_duration_seconds(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        max_duration_seconds = _parse_max_duration_seconds(
            d.pop("max_duration_seconds", UNSET)
        )

        max_images = d.pop("max_images", UNSET)

        def _parse_max_keyframes(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_keyframes = _parse_max_keyframes(d.pop("max_keyframes", UNSET))

        def _parse_max_polycount(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_polycount = _parse_max_polycount(d.pop("max_polycount", UNSET))

        max_reference_images = d.pop("max_reference_images", UNSET)

        native_pixel_art = d.pop("native_pixel_art", UNSET)

        output_formats = cast(List[str], d.pop("output_formats", UNSET))

        def _parse_pixel_size(data: object) -> Union["PixelSize", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                pixel_size_type_1 = PixelSize.from_dict(data)

                return pixel_size_type_1
            except:  # noqa: E722
                pass
            return cast(Union["PixelSize", None, Unset], data)

        pixel_size = _parse_pixel_size(d.pop("pixel_size", UNSET))

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

        def _parse_region_input(data: object) -> Union[None, RegionInput, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                region_input_type_1 = RegionInput(data)

                return region_input_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, RegionInput, Unset], data)

        region_input = _parse_region_input(d.pop("region_input", UNSET))

        segmentation_prompts = []
        _segmentation_prompts = d.pop("segmentation_prompts", UNSET)
        for segmentation_prompts_item_data in _segmentation_prompts or []:
            segmentation_prompts_item = SegmentationPromptType(
                segmentation_prompts_item_data
            )

            segmentation_prompts.append(segmentation_prompts_item)

        supported_sizes = cast(List[str], d.pop("supported_sizes", UNSET))

        supports_animation = d.pop("supports_animation", UNSET)

        supports_audio_toggle = d.pop("supports_audio_toggle", UNSET)

        supports_background_removal = d.pop("supports_background_removal", UNSET)

        supports_expand = d.pop("supports_expand", UNSET)

        supports_first_frame = d.pop("supports_first_frame", UNSET)

        supports_fixed_camera = d.pop("supports_fixed_camera", UNSET)

        supports_forced_palette = d.pop("supports_forced_palette", UNSET)

        supports_image_to_3d = d.pop("supports_image_to_3d", UNSET)

        supports_image_to_video = d.pop("supports_image_to_video", UNSET)

        supports_inpainting = d.pop("supports_inpainting", UNSET)

        supports_joint_naming = d.pop("supports_joint_naming", UNSET)

        supports_keep_uvs = d.pop("supports_keep_uvs", UNSET)

        supports_keyframes = d.pop("supports_keyframes", UNSET)

        supports_last_frame = d.pop("supports_last_frame", UNSET)

        supports_layout = d.pop("supports_layout", UNSET)

        supports_loop = d.pop("supports_loop", UNSET)

        supports_mask_grow = d.pop("supports_mask_grow", UNSET)

        supports_mask_prompt = d.pop("supports_mask_prompt", UNSET)

        supports_multiview_to_3d = d.pop("supports_multiview_to_3d", UNSET)

        supports_palette = d.pop("supports_palette", UNSET)

        supports_pbr = d.pop("supports_pbr", UNSET)

        supports_placement = d.pop("supports_placement", UNSET)

        supports_preserve_unmasked = d.pop("supports_preserve_unmasked", UNSET)

        supports_region_edit = d.pop("supports_region_edit", UNSET)

        supports_rig = d.pop("supports_rig", UNSET)

        supports_segmentation = d.pop("supports_segmentation", UNSET)

        supports_skeleton = d.pop("supports_skeleton", UNSET)

        supports_source_model = d.pop("supports_source_model", UNSET)

        supports_source_video = d.pop("supports_source_video", UNSET)

        supports_text_to_3d = d.pop("supports_text_to_3d", UNSET)

        supports_text_to_video = d.pop("supports_text_to_video", UNSET)

        supports_texture = d.pop("supports_texture", UNSET)

        supports_time_range = d.pop("supports_time_range", UNSET)

        supports_transparent_background = d.pop(
            "supports_transparent_background", UNSET
        )

        texture_resolutions = cast(List[int], d.pop("texture_resolutions", UNSET))

        model_capabilities = cls(
            supports_image_to_image=supports_image_to_image,
            supports_text_to_image=supports_text_to_image,
            task=task,
            directions=directions,
            duration_values=duration_values,
            extend_output=extend_output,
            keyframe_timing=keyframe_timing,
            layout_kinds=layout_kinds,
            lossy_only=lossy_only,
            mask_convention=mask_convention,
            max_duration_seconds=max_duration_seconds,
            max_images=max_images,
            max_keyframes=max_keyframes,
            max_polycount=max_polycount,
            max_reference_images=max_reference_images,
            native_pixel_art=native_pixel_art,
            output_formats=output_formats,
            pixel_size=pixel_size,
            preserves_unmasked=preserves_unmasked,
            region_input=region_input,
            segmentation_prompts=segmentation_prompts,
            supported_sizes=supported_sizes,
            supports_animation=supports_animation,
            supports_audio_toggle=supports_audio_toggle,
            supports_background_removal=supports_background_removal,
            supports_expand=supports_expand,
            supports_first_frame=supports_first_frame,
            supports_fixed_camera=supports_fixed_camera,
            supports_forced_palette=supports_forced_palette,
            supports_image_to_3d=supports_image_to_3d,
            supports_image_to_video=supports_image_to_video,
            supports_inpainting=supports_inpainting,
            supports_joint_naming=supports_joint_naming,
            supports_keep_uvs=supports_keep_uvs,
            supports_keyframes=supports_keyframes,
            supports_last_frame=supports_last_frame,
            supports_layout=supports_layout,
            supports_loop=supports_loop,
            supports_mask_grow=supports_mask_grow,
            supports_mask_prompt=supports_mask_prompt,
            supports_multiview_to_3d=supports_multiview_to_3d,
            supports_palette=supports_palette,
            supports_pbr=supports_pbr,
            supports_placement=supports_placement,
            supports_preserve_unmasked=supports_preserve_unmasked,
            supports_region_edit=supports_region_edit,
            supports_rig=supports_rig,
            supports_segmentation=supports_segmentation,
            supports_skeleton=supports_skeleton,
            supports_source_model=supports_source_model,
            supports_source_video=supports_source_video,
            supports_text_to_3d=supports_text_to_3d,
            supports_text_to_video=supports_text_to_video,
            supports_texture=supports_texture,
            supports_time_range=supports_time_range,
            supports_transparent_background=supports_transparent_background,
            texture_resolutions=texture_resolutions,
        )

        model_capabilities.additional_properties = d
        return model_capabilities

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
