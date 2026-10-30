from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecExpand")


@_attrs_define
class ParamSpecExpand:
    """`expand` on a model (3D doc §4): how its vendor grows a canvas.

    Attributes:
        centered_only (Union[Unset, bool]): The vendor only centres the source: left must equal right, and top
            bottom (anything else is `expand_unsupported`).
        description (Union[None, Unset, str]):
        label (Union[None, Unset, str]):
        max_px (Union[None, Unset, int]): The most pixels it adds to one side.
    """

    centered_only: Union[Unset, bool] = UNSET
    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_px: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        centered_only = self.centered_only

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

        max_px: Union[None, Unset, int]
        if isinstance(self.max_px, Unset):
            max_px = UNSET
        else:
            max_px = self.max_px

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if centered_only is not UNSET:
            field_dict["centered_only"] = centered_only
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if max_px is not UNSET:
            field_dict["max_px"] = max_px

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        centered_only = d.pop("centered_only", UNSET)

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

        def _parse_max_px(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_px = _parse_max_px(d.pop("max_px", UNSET))

        param_spec_expand = cls(
            centered_only=centered_only,
            description=description,
            label=label,
            max_px=max_px,
        )

        param_spec_expand.additional_properties = d
        return param_spec_expand

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
