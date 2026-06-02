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
        supported_sizes (Union[Unset, List[str]]):
        supports_first_frame (Union[Unset, bool]):
        supports_image_to_video (Union[Unset, bool]):
        supports_inpainting (Union[Unset, bool]):
        supports_last_frame (Union[Unset, bool]):
        supports_text_to_video (Union[Unset, bool]):
    """

    supports_image_to_image: bool
    supports_text_to_image: bool
    max_duration_seconds: Union[None, Unset, float] = UNSET
    max_images: Union[Unset, int] = UNSET
    supported_sizes: Union[Unset, List[str]] = UNSET
    supports_first_frame: Union[Unset, bool] = UNSET
    supports_image_to_video: Union[Unset, bool] = UNSET
    supports_inpainting: Union[Unset, bool] = UNSET
    supports_last_frame: Union[Unset, bool] = UNSET
    supports_text_to_video: Union[Unset, bool] = UNSET
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

        supported_sizes: Union[Unset, List[str]] = UNSET
        if not isinstance(self.supported_sizes, Unset):
            supported_sizes = self.supported_sizes

        supports_first_frame = self.supports_first_frame

        supports_image_to_video = self.supports_image_to_video

        supports_inpainting = self.supports_inpainting

        supports_last_frame = self.supports_last_frame

        supports_text_to_video = self.supports_text_to_video

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
        if supported_sizes is not UNSET:
            field_dict["supported_sizes"] = supported_sizes
        if supports_first_frame is not UNSET:
            field_dict["supports_first_frame"] = supports_first_frame
        if supports_image_to_video is not UNSET:
            field_dict["supports_image_to_video"] = supports_image_to_video
        if supports_inpainting is not UNSET:
            field_dict["supports_inpainting"] = supports_inpainting
        if supports_last_frame is not UNSET:
            field_dict["supports_last_frame"] = supports_last_frame
        if supports_text_to_video is not UNSET:
            field_dict["supports_text_to_video"] = supports_text_to_video

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

        supported_sizes = cast(List[str], d.pop("supported_sizes", UNSET))

        supports_first_frame = d.pop("supports_first_frame", UNSET)

        supports_image_to_video = d.pop("supports_image_to_video", UNSET)

        supports_inpainting = d.pop("supports_inpainting", UNSET)

        supports_last_frame = d.pop("supports_last_frame", UNSET)

        supports_text_to_video = d.pop("supports_text_to_video", UNSET)

        model_capabilities = cls(
            supports_image_to_image=supports_image_to_image,
            supports_text_to_image=supports_text_to_image,
            max_duration_seconds=max_duration_seconds,
            max_images=max_images,
            supported_sizes=supported_sizes,
            supports_first_frame=supports_first_frame,
            supports_image_to_video=supports_image_to_video,
            supports_inpainting=supports_inpainting,
            supports_last_frame=supports_last_frame,
            supports_text_to_video=supports_text_to_video,
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
