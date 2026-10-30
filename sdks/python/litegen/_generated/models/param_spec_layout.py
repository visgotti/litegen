from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.layout_kind import LayoutKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecLayout")


@_attrs_define
class ParamSpecLayout:
    """`layout` on a text-to-image model (3D doc §4): the boxes it composes.

    Attributes:
        kinds (List[LayoutKind]): What its boxes may hold, listed as `layout_kinds`.
        max_items (int): The most boxes one request may send.
        description (Union[None, Unset, str]):
        label (Union[None, Unset, str]):
        single_word_text (Union[Unset, bool]): Each text box holds one word (Recraft's `text_layout`).
    """

    kinds: List[LayoutKind]
    max_items: int
    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    single_word_text: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        kinds = []
        for kinds_item_data in self.kinds:
            kinds_item = kinds_item_data.value
            kinds.append(kinds_item)

        max_items = self.max_items

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

        single_word_text = self.single_word_text

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "kinds": kinds,
                "max_items": max_items,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if single_word_text is not UNSET:
            field_dict["single_word_text"] = single_word_text

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        kinds = []
        _kinds = d.pop("kinds")
        for kinds_item_data in _kinds:
            kinds_item = LayoutKind(kinds_item_data)

            kinds.append(kinds_item)

        max_items = d.pop("max_items")

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

        single_word_text = d.pop("single_word_text", UNSET)

        param_spec_layout = cls(
            kinds=kinds,
            max_items=max_items,
            description=description,
            label=label,
            single_word_text=single_word_text,
        )

        param_spec_layout.additional_properties = d
        return param_spec_layout

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
