from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecPalette")


@_attrs_define
class ParamSpecPalette:
    """`palette` on an image model (PixelGenie L16): the colours it keeps to.

    Attributes:
        max_colors (int): The most colours one palette may hold.
        description (Union[None, Unset, str]):
        forced (Union[Unset, bool]): Every pixel is one of the palette's colours (PixelLab's forced
            palette, Retro Diffusion's `input_palette`); otherwise the palette is
            a hint (Ideogram's `color_palette`, Recraft's `controls`).
        label (Union[None, Unset, str]):
    """

    max_colors: int
    description: Union[None, Unset, str] = UNSET
    forced: Union[Unset, bool] = UNSET
    label: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_colors = self.max_colors

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        forced = self.forced

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_colors": max_colors,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if forced is not UNSET:
            field_dict["forced"] = forced
        if label is not UNSET:
            field_dict["label"] = label

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_colors = d.pop("max_colors")

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        forced = d.pop("forced", UNSET)

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        param_spec_palette = cls(
            max_colors=max_colors,
            description=description,
            forced=forced,
            label=label,
        )

        param_spec_palette.additional_properties = d
        return param_spec_palette

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
