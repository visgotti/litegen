from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AnimationPresetSpec")


@_attrs_define
class AnimationPresetSpec:
    """
    Attributes:
        id (str): The vendor's own id for the clip.
        canonical (Union[None, Unset, str]): The canonical preset it plays, when it has one.
        label (Union[None, Unset, str]): The vendor's own name for it, for people choosing one.
        loopable (Union[Unset, bool]): Whether the vendor made it to loop.
    """

    id: str
    canonical: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    loopable: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        id = self.id

        canonical: Union[None, Unset, str]
        if isinstance(self.canonical, Unset):
            canonical = UNSET
        else:
            canonical = self.canonical

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        loopable = self.loopable

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
            }
        )
        if canonical is not UNSET:
            field_dict["canonical"] = canonical
        if label is not UNSET:
            field_dict["label"] = label
        if loopable is not UNSET:
            field_dict["loopable"] = loopable

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        id = d.pop("id")

        def _parse_canonical(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        canonical = _parse_canonical(d.pop("canonical", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        loopable = d.pop("loopable", UNSET)

        animation_preset_spec = cls(
            id=id,
            canonical=canonical,
            label=label,
            loopable=loopable,
        )

        animation_preset_spec.additional_properties = d
        return animation_preset_spec

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
