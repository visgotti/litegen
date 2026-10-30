from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.keyframe_timing import KeyframeTiming
from ..models.mask_convention import MaskConvention
from ..models.model_task import ModelTask
from ..models.preserves_unmasked import PreservesUnmasked
from ..models.region_input import RegionInput
from ..types import UNSET, Unset

T = TypeVar("T", bound="ModelCapabilities")


@_attrs_define
class ModelCapabilities:
    """
    Attributes:
        supports_image_to_image (bool):
        supports_text_to_image (bool):
        duration_values (Union[Unset, List[float]]): The only `duration_seconds` values the model takes; empty when it
            takes its range.
        keyframe_timing (Union[KeyframeTiming, None, Unset]):
        lossy_only (Union[Unset, bool]): Returns only lossy containers (no PNG).
        mask_convention (Union[MaskConvention, None, Unset]):
        max_duration_seconds (Union[None, Unset, float]): The longest clip the model makes, from its `duration_seconds`
            param.
        max_images (Union[Unset, int]):
        max_keyframes (Union[None, Unset, int]): The most `keyframe` references one request may carry.
        max_polycount (Union[None, Unset, int]): Upper bound of the model's `target_polycount` param, when it takes one.
        max_reference_images (Union[Unset, int]): The most reference images one request may carry. Kept under its old
            name too: `max_images` is this same number (L14).
        output_formats (Union[Unset, List[str]]): Every mesh container a caller may request, including those litegen
            derives locally — the same list, in the same order, as
            `GET /v1/models/{id}` serves under `params.output_formats.enum_values`.
            Empty for image and video models.
        preserves_unmasked (Union[None, PreservesUnmasked, Unset]):
        region_input (Union[None, RegionInput, Unset]):
        supported_sizes (Union[Unset, List[str]]):
        supports_audio_toggle (Union[Unset, bool]): Takes `audio: false` (or `true`) to leave audio out (or in).
        supports_first_frame (Union[Unset, bool]):
        supports_fixed_camera (Union[Unset, bool]): Takes `camera: fixed` (PixelGenie L10).
        supports_image_to_3d (Union[Unset, bool]): Accepts one `init` reference image (image-to-3D).
        supports_image_to_video (Union[Unset, bool]):
        supports_inpainting (Union[Unset, bool]):
        supports_keyframes (Union[Unset, bool]): Takes `keyframe` references, each pinned at its `time_s` (3D doc §5).
        supports_last_frame (Union[Unset, bool]):
        supports_loop (Union[Unset, bool]): Takes `loop: true`.
        supports_multiview_to_3d (Union[Unset, bool]): Accepts several `view-*` reference images (multiview-to-3D).
        supports_pbr (Union[Unset, bool]): Takes the `pbr` param.
        supports_region_edit (Union[Unset, bool]): Takes `region` (3D doc §4).
        supports_rig (Union[Unset, bool]): Takes the `rig` param.
        supports_source_model (Union[Unset, bool]): Takes `source_model`: an operation on an existing mesh.
        supports_text_to_3d (Union[Unset, bool]): Accepts a prompt with no reference images (text-to-3D).
        supports_text_to_video (Union[Unset, bool]):
        supports_texture (Union[Unset, bool]): Takes the `texture` param.
        supports_transparent_background (Union[Unset, bool]): Takes `background: transparent` (PixelGenie L9).
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

    supports_image_to_image: bool
    supports_text_to_image: bool
    duration_values: Union[Unset, List[float]] = UNSET
    keyframe_timing: Union[KeyframeTiming, None, Unset] = UNSET
    lossy_only: Union[Unset, bool] = UNSET
    mask_convention: Union[MaskConvention, None, Unset] = UNSET
    max_duration_seconds: Union[None, Unset, float] = UNSET
    max_images: Union[Unset, int] = UNSET
    max_keyframes: Union[None, Unset, int] = UNSET
    max_polycount: Union[None, Unset, int] = UNSET
    max_reference_images: Union[Unset, int] = UNSET
    output_formats: Union[Unset, List[str]] = UNSET
    preserves_unmasked: Union[None, PreservesUnmasked, Unset] = UNSET
    region_input: Union[None, RegionInput, Unset] = UNSET
    supported_sizes: Union[Unset, List[str]] = UNSET
    supports_audio_toggle: Union[Unset, bool] = UNSET
    supports_first_frame: Union[Unset, bool] = UNSET
    supports_fixed_camera: Union[Unset, bool] = UNSET
    supports_image_to_3d: Union[Unset, bool] = UNSET
    supports_image_to_video: Union[Unset, bool] = UNSET
    supports_inpainting: Union[Unset, bool] = UNSET
    supports_keyframes: Union[Unset, bool] = UNSET
    supports_last_frame: Union[Unset, bool] = UNSET
    supports_loop: Union[Unset, bool] = UNSET
    supports_multiview_to_3d: Union[Unset, bool] = UNSET
    supports_pbr: Union[Unset, bool] = UNSET
    supports_region_edit: Union[Unset, bool] = UNSET
    supports_rig: Union[Unset, bool] = UNSET
    supports_source_model: Union[Unset, bool] = UNSET
    supports_text_to_3d: Union[Unset, bool] = UNSET
    supports_text_to_video: Union[Unset, bool] = UNSET
    supports_texture: Union[Unset, bool] = UNSET
    supports_transparent_background: Union[Unset, bool] = UNSET
    task: Union[Unset, ModelTask] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        supports_image_to_image = self.supports_image_to_image

        supports_text_to_image = self.supports_text_to_image

        duration_values: Union[Unset, List[float]] = UNSET
        if not isinstance(self.duration_values, Unset):
            duration_values = self.duration_values

        keyframe_timing: Union[None, Unset, str]
        if isinstance(self.keyframe_timing, Unset):
            keyframe_timing = UNSET
        elif isinstance(self.keyframe_timing, KeyframeTiming):
            keyframe_timing = self.keyframe_timing.value
        else:
            keyframe_timing = self.keyframe_timing

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

        output_formats: Union[Unset, List[str]] = UNSET
        if not isinstance(self.output_formats, Unset):
            output_formats = self.output_formats

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

        supported_sizes: Union[Unset, List[str]] = UNSET
        if not isinstance(self.supported_sizes, Unset):
            supported_sizes = self.supported_sizes

        supports_audio_toggle = self.supports_audio_toggle

        supports_first_frame = self.supports_first_frame

        supports_fixed_camera = self.supports_fixed_camera

        supports_image_to_3d = self.supports_image_to_3d

        supports_image_to_video = self.supports_image_to_video

        supports_inpainting = self.supports_inpainting

        supports_keyframes = self.supports_keyframes

        supports_last_frame = self.supports_last_frame

        supports_loop = self.supports_loop

        supports_multiview_to_3d = self.supports_multiview_to_3d

        supports_pbr = self.supports_pbr

        supports_region_edit = self.supports_region_edit

        supports_rig = self.supports_rig

        supports_source_model = self.supports_source_model

        supports_text_to_3d = self.supports_text_to_3d

        supports_text_to_video = self.supports_text_to_video

        supports_texture = self.supports_texture

        supports_transparent_background = self.supports_transparent_background

        task: Union[Unset, str] = UNSET
        if not isinstance(self.task, Unset):
            task = self.task.value

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "supports_image_to_image": supports_image_to_image,
                "supports_text_to_image": supports_text_to_image,
            }
        )
        if duration_values is not UNSET:
            field_dict["duration_values"] = duration_values
        if keyframe_timing is not UNSET:
            field_dict["keyframe_timing"] = keyframe_timing
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
        if output_formats is not UNSET:
            field_dict["output_formats"] = output_formats
        if preserves_unmasked is not UNSET:
            field_dict["preserves_unmasked"] = preserves_unmasked
        if region_input is not UNSET:
            field_dict["region_input"] = region_input
        if supported_sizes is not UNSET:
            field_dict["supported_sizes"] = supported_sizes
        if supports_audio_toggle is not UNSET:
            field_dict["supports_audio_toggle"] = supports_audio_toggle
        if supports_first_frame is not UNSET:
            field_dict["supports_first_frame"] = supports_first_frame
        if supports_fixed_camera is not UNSET:
            field_dict["supports_fixed_camera"] = supports_fixed_camera
        if supports_image_to_3d is not UNSET:
            field_dict["supports_image_to_3d"] = supports_image_to_3d
        if supports_image_to_video is not UNSET:
            field_dict["supports_image_to_video"] = supports_image_to_video
        if supports_inpainting is not UNSET:
            field_dict["supports_inpainting"] = supports_inpainting
        if supports_keyframes is not UNSET:
            field_dict["supports_keyframes"] = supports_keyframes
        if supports_last_frame is not UNSET:
            field_dict["supports_last_frame"] = supports_last_frame
        if supports_loop is not UNSET:
            field_dict["supports_loop"] = supports_loop
        if supports_multiview_to_3d is not UNSET:
            field_dict["supports_multiview_to_3d"] = supports_multiview_to_3d
        if supports_pbr is not UNSET:
            field_dict["supports_pbr"] = supports_pbr
        if supports_region_edit is not UNSET:
            field_dict["supports_region_edit"] = supports_region_edit
        if supports_rig is not UNSET:
            field_dict["supports_rig"] = supports_rig
        if supports_source_model is not UNSET:
            field_dict["supports_source_model"] = supports_source_model
        if supports_text_to_3d is not UNSET:
            field_dict["supports_text_to_3d"] = supports_text_to_3d
        if supports_text_to_video is not UNSET:
            field_dict["supports_text_to_video"] = supports_text_to_video
        if supports_texture is not UNSET:
            field_dict["supports_texture"] = supports_texture
        if supports_transparent_background is not UNSET:
            field_dict["supports_transparent_background"] = (
                supports_transparent_background
            )
        if task is not UNSET:
            field_dict["task"] = task

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        supports_image_to_image = d.pop("supports_image_to_image")

        supports_text_to_image = d.pop("supports_text_to_image")

        duration_values = cast(List[float], d.pop("duration_values", UNSET))

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

        output_formats = cast(List[str], d.pop("output_formats", UNSET))

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

        supported_sizes = cast(List[str], d.pop("supported_sizes", UNSET))

        supports_audio_toggle = d.pop("supports_audio_toggle", UNSET)

        supports_first_frame = d.pop("supports_first_frame", UNSET)

        supports_fixed_camera = d.pop("supports_fixed_camera", UNSET)

        supports_image_to_3d = d.pop("supports_image_to_3d", UNSET)

        supports_image_to_video = d.pop("supports_image_to_video", UNSET)

        supports_inpainting = d.pop("supports_inpainting", UNSET)

        supports_keyframes = d.pop("supports_keyframes", UNSET)

        supports_last_frame = d.pop("supports_last_frame", UNSET)

        supports_loop = d.pop("supports_loop", UNSET)

        supports_multiview_to_3d = d.pop("supports_multiview_to_3d", UNSET)

        supports_pbr = d.pop("supports_pbr", UNSET)

        supports_region_edit = d.pop("supports_region_edit", UNSET)

        supports_rig = d.pop("supports_rig", UNSET)

        supports_source_model = d.pop("supports_source_model", UNSET)

        supports_text_to_3d = d.pop("supports_text_to_3d", UNSET)

        supports_text_to_video = d.pop("supports_text_to_video", UNSET)

        supports_texture = d.pop("supports_texture", UNSET)

        supports_transparent_background = d.pop(
            "supports_transparent_background", UNSET
        )

        _task = d.pop("task", UNSET)
        task: Union[Unset, ModelTask]
        if isinstance(_task, Unset):
            task = UNSET
        else:
            task = ModelTask(_task)

        model_capabilities = cls(
            supports_image_to_image=supports_image_to_image,
            supports_text_to_image=supports_text_to_image,
            duration_values=duration_values,
            keyframe_timing=keyframe_timing,
            lossy_only=lossy_only,
            mask_convention=mask_convention,
            max_duration_seconds=max_duration_seconds,
            max_images=max_images,
            max_keyframes=max_keyframes,
            max_polycount=max_polycount,
            max_reference_images=max_reference_images,
            output_formats=output_formats,
            preserves_unmasked=preserves_unmasked,
            region_input=region_input,
            supported_sizes=supported_sizes,
            supports_audio_toggle=supports_audio_toggle,
            supports_first_frame=supports_first_frame,
            supports_fixed_camera=supports_fixed_camera,
            supports_image_to_3d=supports_image_to_3d,
            supports_image_to_video=supports_image_to_video,
            supports_inpainting=supports_inpainting,
            supports_keyframes=supports_keyframes,
            supports_last_frame=supports_last_frame,
            supports_loop=supports_loop,
            supports_multiview_to_3d=supports_multiview_to_3d,
            supports_pbr=supports_pbr,
            supports_region_edit=supports_region_edit,
            supports_rig=supports_rig,
            supports_source_model=supports_source_model,
            supports_text_to_3d=supports_text_to_3d,
            supports_text_to_video=supports_text_to_video,
            supports_texture=supports_texture,
            supports_transparent_background=supports_transparent_background,
            task=task,
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
