from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ModelCapabilities")


@_attrs_define
class ModelCapabilities:
    """
    Attributes:
        supports_image_to_image (bool):
        supports_text_to_image (bool):
        max_duration_seconds (Union[None, Unset, float]):
        max_images (Union[Unset, int]):
        max_polycount (Union[None, Unset, int]):
        output_formats (Union[Unset, List[str]]):
        supported_sizes (Union[Unset, List[str]]):
        supports_first_frame (Union[Unset, bool]):
        supports_image_to_3d (Union[Unset, bool]):
        supports_image_to_video (Union[Unset, bool]):
        supports_inpainting (Union[Unset, bool]):
        supports_last_frame (Union[Unset, bool]):
        supports_multiview_to_3d (Union[Unset, bool]):
        supports_pbr (Union[Unset, bool]):
        supports_rig (Union[Unset, bool]):
        supports_text_to_3d (Union[Unset, bool]):
        supports_text_to_video (Union[Unset, bool]):
        supports_texture (Union[Unset, bool]):
    """

    supports_image_to_image: bool
    supports_text_to_image: bool
    max_duration_seconds: Union[None, Unset, float] = UNSET
    max_images: Union[Unset, int] = UNSET
    max_polycount: Union[None, Unset, int] = UNSET
    output_formats: Union[Unset, List[str]] = UNSET
    supported_sizes: Union[Unset, List[str]] = UNSET
    supports_first_frame: Union[Unset, bool] = UNSET
    supports_image_to_3d: Union[Unset, bool] = UNSET
    supports_image_to_video: Union[Unset, bool] = UNSET
    supports_inpainting: Union[Unset, bool] = UNSET
    supports_last_frame: Union[Unset, bool] = UNSET
    supports_multiview_to_3d: Union[Unset, bool] = UNSET
    supports_pbr: Union[Unset, bool] = UNSET
    supports_rig: Union[Unset, bool] = UNSET
    supports_text_to_3d: Union[Unset, bool] = UNSET
    supports_text_to_video: Union[Unset, bool] = UNSET
    supports_texture: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        supports_image_to_image = self.supports_image_to_image

        supports_text_to_image = self.supports_text_to_image

        max_duration_seconds: Union[None, Unset, float]
        if isinstance(self.max_duration_seconds, Unset):
            max_duration_seconds = UNSET
        else:
            max_duration_seconds = self.max_duration_seconds

        max_images = self.max_images

        max_polycount: Union[None, Unset, int]
        if isinstance(self.max_polycount, Unset):
            max_polycount = UNSET
        else:
            max_polycount = self.max_polycount

        output_formats: Union[Unset, List[str]] = UNSET
        if not isinstance(self.output_formats, Unset):
            output_formats = self.output_formats

        supported_sizes: Union[Unset, List[str]] = UNSET
        if not isinstance(self.supported_sizes, Unset):
            supported_sizes = self.supported_sizes

        supports_first_frame = self.supports_first_frame

        supports_image_to_3d = self.supports_image_to_3d

        supports_image_to_video = self.supports_image_to_video

        supports_inpainting = self.supports_inpainting

        supports_last_frame = self.supports_last_frame

        supports_multiview_to_3d = self.supports_multiview_to_3d

        supports_pbr = self.supports_pbr

        supports_rig = self.supports_rig

        supports_text_to_3d = self.supports_text_to_3d

        supports_text_to_video = self.supports_text_to_video

        supports_texture = self.supports_texture

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "supports_image_to_image": supports_image_to_image,
                "supports_text_to_image": supports_text_to_image,
            }
        )
        if max_duration_seconds is not UNSET:
            field_dict["max_duration_seconds"] = max_duration_seconds
        if max_images is not UNSET:
            field_dict["max_images"] = max_images
        if max_polycount is not UNSET:
            field_dict["max_polycount"] = max_polycount
        if output_formats is not UNSET:
            field_dict["output_formats"] = output_formats
        if supported_sizes is not UNSET:
            field_dict["supported_sizes"] = supported_sizes
        if supports_first_frame is not UNSET:
            field_dict["supports_first_frame"] = supports_first_frame
        if supports_image_to_3d is not UNSET:
            field_dict["supports_image_to_3d"] = supports_image_to_3d
        if supports_image_to_video is not UNSET:
            field_dict["supports_image_to_video"] = supports_image_to_video
        if supports_inpainting is not UNSET:
            field_dict["supports_inpainting"] = supports_inpainting
        if supports_last_frame is not UNSET:
            field_dict["supports_last_frame"] = supports_last_frame
        if supports_multiview_to_3d is not UNSET:
            field_dict["supports_multiview_to_3d"] = supports_multiview_to_3d
        if supports_pbr is not UNSET:
            field_dict["supports_pbr"] = supports_pbr
        if supports_rig is not UNSET:
            field_dict["supports_rig"] = supports_rig
        if supports_text_to_3d is not UNSET:
            field_dict["supports_text_to_3d"] = supports_text_to_3d
        if supports_text_to_video is not UNSET:
            field_dict["supports_text_to_video"] = supports_text_to_video
        if supports_texture is not UNSET:
            field_dict["supports_texture"] = supports_texture

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        supports_image_to_image = d.pop("supports_image_to_image")

        supports_text_to_image = d.pop("supports_text_to_image")

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

        def _parse_max_polycount(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_polycount = _parse_max_polycount(d.pop("max_polycount", UNSET))

        output_formats = cast(List[str], d.pop("output_formats", UNSET))

        supported_sizes = cast(List[str], d.pop("supported_sizes", UNSET))

        supports_first_frame = d.pop("supports_first_frame", UNSET)

        supports_image_to_3d = d.pop("supports_image_to_3d", UNSET)

        supports_image_to_video = d.pop("supports_image_to_video", UNSET)

        supports_inpainting = d.pop("supports_inpainting", UNSET)

        supports_last_frame = d.pop("supports_last_frame", UNSET)

        supports_multiview_to_3d = d.pop("supports_multiview_to_3d", UNSET)

        supports_pbr = d.pop("supports_pbr", UNSET)

        supports_rig = d.pop("supports_rig", UNSET)

        supports_text_to_3d = d.pop("supports_text_to_3d", UNSET)

        supports_text_to_video = d.pop("supports_text_to_video", UNSET)

        supports_texture = d.pop("supports_texture", UNSET)

        model_capabilities = cls(
            supports_image_to_image=supports_image_to_image,
            supports_text_to_image=supports_text_to_image,
            max_duration_seconds=max_duration_seconds,
            max_images=max_images,
            max_polycount=max_polycount,
            output_formats=output_formats,
            supported_sizes=supported_sizes,
            supports_first_frame=supports_first_frame,
            supports_image_to_3d=supports_image_to_3d,
            supports_image_to_video=supports_image_to_video,
            supports_inpainting=supports_inpainting,
            supports_last_frame=supports_last_frame,
            supports_multiview_to_3d=supports_multiview_to_3d,
            supports_pbr=supports_pbr,
            supports_rig=supports_rig,
            supports_text_to_3d=supports_text_to_3d,
            supports_text_to_video=supports_text_to_video,
            supports_texture=supports_texture,
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
