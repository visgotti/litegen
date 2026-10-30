from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.segmentation_prompt_type import SegmentationPromptType
from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecSegmentationPrompts")


@_attrs_define
class ParamSpecSegmentationPrompts:
    """`prompts` on a segmentation model (PixelGenie L7): what it can be asked
    to select.

        Attributes:
            max_items (int): The most prompts one request may send.
            max_masks (int): The most masks one answer holds: a text prompt finds at most this
                many instances.
            types (List[SegmentationPromptType]): The prompt types the model takes, listed as `segmentation_prompts`.
            description (Union[None, Unset, str]):
            label (Union[None, Unset, str]):
    """

    max_items: int
    max_masks: int
    types: List[SegmentationPromptType]
    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_items = self.max_items

        max_masks = self.max_masks

        types = []
        for types_item_data in self.types:
            types_item = types_item_data.value
            types.append(types_item)

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_items": max_items,
                "max_masks": max_masks,
                "types": types,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_items = d.pop("max_items")

        max_masks = d.pop("max_masks")

        types = []
        _types = d.pop("types")
        for types_item_data in _types:
            types_item = SegmentationPromptType(types_item_data)

            types.append(types_item)

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        param_spec_segmentation_prompts = cls(
            max_items=max_items,
            max_masks=max_masks,
            types=types,
            description=description,
            label=label,
        )

        param_spec_segmentation_prompts.additional_properties = d
        return param_spec_segmentation_prompts

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
