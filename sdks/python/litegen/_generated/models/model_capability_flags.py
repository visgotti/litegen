from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ModelCapabilityFlags")


@_attrs_define
class ModelCapabilityFlags:
    """
    Attributes:
        image_to_image (Union[Unset, bool]):
        image_to_video (Union[Unset, bool]):
        inpainting (Union[Unset, bool]):
        text_to_image (Union[Unset, bool]):
        text_to_video (Union[Unset, bool]):
    """

    image_to_image: Union[Unset, bool] = UNSET
    image_to_video: Union[Unset, bool] = UNSET
    inpainting: Union[Unset, bool] = UNSET
    text_to_image: Union[Unset, bool] = UNSET
    text_to_video: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        image_to_image = self.image_to_image

        image_to_video = self.image_to_video

        inpainting = self.inpainting

        text_to_image = self.text_to_image

        text_to_video = self.text_to_video

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if image_to_image is not UNSET:
            field_dict["image_to_image"] = image_to_image
        if image_to_video is not UNSET:
            field_dict["image_to_video"] = image_to_video
        if inpainting is not UNSET:
            field_dict["inpainting"] = inpainting
        if text_to_image is not UNSET:
            field_dict["text_to_image"] = text_to_image
        if text_to_video is not UNSET:
            field_dict["text_to_video"] = text_to_video

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        image_to_image = d.pop("image_to_image", UNSET)

        image_to_video = d.pop("image_to_video", UNSET)

        inpainting = d.pop("inpainting", UNSET)

        text_to_image = d.pop("text_to_image", UNSET)

        text_to_video = d.pop("text_to_video", UNSET)

        model_capability_flags = cls(
            image_to_image=image_to_image,
            image_to_video=image_to_video,
            inpainting=inpainting,
            text_to_image=text_to_image,
            text_to_video=text_to_video,
        )

        model_capability_flags.additional_properties = d
        return model_capability_flags

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
