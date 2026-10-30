from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.layout_kind import LayoutKind
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.layout_box import LayoutBox


T = TypeVar("T", bound="LayoutItem")


@_attrs_define
class LayoutItem:
    """One box of a text-to-image layout (3D doc §4). Boxes guide the layout;
    no vendor clips to them.

        Attributes:
            box (LayoutBox): Where a layout box sits, as fractions of the canvas (0 to 1, top-left
                origin): the output's size may not be known yet.
            kind (LayoutKind): What a layout box holds (3D doc §4).
            prompt (Union[None, Unset, str]): What goes there: required for an `object` box.
            text (Union[None, Unset, str]): The words to render: required for a `text` box.
    """

    box: "LayoutBox"
    kind: LayoutKind
    prompt: Union[None, Unset, str] = UNSET
    text: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        box = self.box.to_dict()

        kind = self.kind.value

        prompt: Union[None, Unset, str]
        if isinstance(self.prompt, Unset):
            prompt = UNSET
        else:
            prompt = self.prompt

        text: Union[None, Unset, str]
        if isinstance(self.text, Unset):
            text = UNSET
        else:
            text = self.text

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "box": box,
                "kind": kind,
            }
        )
        if prompt is not UNSET:
            field_dict["prompt"] = prompt
        if text is not UNSET:
            field_dict["text"] = text

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.layout_box import LayoutBox

        d = src_dict.copy()
        box = LayoutBox.from_dict(d.pop("box"))

        kind = LayoutKind(d.pop("kind"))

        def _parse_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        prompt = _parse_prompt(d.pop("prompt", UNSET))

        def _parse_text(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        text = _parse_text(d.pop("text", UNSET))

        layout_item = cls(
            box=box,
            kind=kind,
            prompt=prompt,
            text=text,
        )

        layout_item.additional_properties = d
        return layout_item

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
