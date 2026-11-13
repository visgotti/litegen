from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.image_segmentation_request_metadata_type_0 import (
        ImageSegmentationRequestMetadataType0,
    )
    from ..models.segmentation_prompt_type_0 import SegmentationPromptType0
    from ..models.segmentation_prompt_type_1 import SegmentationPromptType1
    from ..models.segmentation_prompt_type_2 import SegmentationPromptType2


T = TypeVar("T", bound="ImageSegmentationRequest")


@_attrs_define
class ImageSegmentationRequest:
    """`POST /v1/images/segmentations` (PixelGenie L7). Answered at once.

    Attributes:
        image (str): The image: base64, or a `data:` URI. PNG, JPEG, WebP or GIF.
        model (str): A model whose `task` is `segment`.
        prompts (List[Union['SegmentationPromptType0', 'SegmentationPromptType1', 'SegmentationPromptType2']]): What to
            select. Point and box prompts together select one object;
            a text prompt selects every instance of what it names.
        metadata (Union['ImageSegmentationRequestMetadataType0', None, Unset]): Your own labels for the call, as on a
            generation (L17).
        strict (Union[Unset, bool]): `true` (the default) refuses a field the request has no such name for
            (`prompt` for `prompts`, say) with `param_unsupported`; `false` drops
            it and names it in `X-Litegen-Dropped-Params`.
    """

    image: str
    model: str
    prompts: List[
        Union[
            "SegmentationPromptType0",
            "SegmentationPromptType1",
            "SegmentationPromptType2",
        ]
    ]
    metadata: Union["ImageSegmentationRequestMetadataType0", None, Unset] = UNSET
    strict: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.image_segmentation_request_metadata_type_0 import (
            ImageSegmentationRequestMetadataType0,
        )
        from ..models.segmentation_prompt_type_0 import SegmentationPromptType0
        from ..models.segmentation_prompt_type_1 import SegmentationPromptType1

        image = self.image

        model = self.model

        prompts = []
        for prompts_item_data in self.prompts:
            prompts_item: Dict[str, Any]
            if isinstance(prompts_item_data, SegmentationPromptType0):
                prompts_item = prompts_item_data.to_dict()
            elif isinstance(prompts_item_data, SegmentationPromptType1):
                prompts_item = prompts_item_data.to_dict()
            else:
                prompts_item = prompts_item_data.to_dict()

            prompts.append(prompts_item)

        metadata: Union[Dict[str, Any], None, Unset]
        if isinstance(self.metadata, Unset):
            metadata = UNSET
        elif isinstance(self.metadata, ImageSegmentationRequestMetadataType0):
            metadata = self.metadata.to_dict()
        else:
            metadata = self.metadata

        strict = self.strict

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "image": image,
                "model": model,
                "prompts": prompts,
            }
        )
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if strict is not UNSET:
            field_dict["strict"] = strict

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.image_segmentation_request_metadata_type_0 import (
            ImageSegmentationRequestMetadataType0,
        )
        from ..models.segmentation_prompt_type_0 import SegmentationPromptType0
        from ..models.segmentation_prompt_type_1 import SegmentationPromptType1
        from ..models.segmentation_prompt_type_2 import SegmentationPromptType2

        d = src_dict.copy()
        image = d.pop("image")

        model = d.pop("model")

        prompts = []
        _prompts = d.pop("prompts")
        for prompts_item_data in _prompts:

            def _parse_prompts_item(
                data: object,
            ) -> Union[
                "SegmentationPromptType0",
                "SegmentationPromptType1",
                "SegmentationPromptType2",
            ]:
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_segmentation_prompt_type_0 = (
                        SegmentationPromptType0.from_dict(data)
                    )

                    return componentsschemas_segmentation_prompt_type_0
                except:  # noqa: E722
                    pass
                try:
                    if not isinstance(data, dict):
                        raise TypeError()
                    componentsschemas_segmentation_prompt_type_1 = (
                        SegmentationPromptType1.from_dict(data)
                    )

                    return componentsschemas_segmentation_prompt_type_1
                except:  # noqa: E722
                    pass
                if not isinstance(data, dict):
                    raise TypeError()
                componentsschemas_segmentation_prompt_type_2 = (
                    SegmentationPromptType2.from_dict(data)
                )

                return componentsschemas_segmentation_prompt_type_2

            prompts_item = _parse_prompts_item(prompts_item_data)

            prompts.append(prompts_item)

        def _parse_metadata(
            data: object,
        ) -> Union["ImageSegmentationRequestMetadataType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                metadata_type_0 = ImageSegmentationRequestMetadataType0.from_dict(data)

                return metadata_type_0
            except:  # noqa: E722
                pass
            return cast(
                Union["ImageSegmentationRequestMetadataType0", None, Unset], data
            )

        metadata = _parse_metadata(d.pop("metadata", UNSET))

        strict = d.pop("strict", UNSET)

        image_segmentation_request = cls(
            image=image,
            model=model,
            prompts=prompts,
            metadata=metadata,
            strict=strict,
        )

        image_segmentation_request.additional_properties = d
        return image_segmentation_request

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
