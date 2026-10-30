from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.region_input import RegionInput
from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecRegion")


@_attrs_define
class ParamSpecRegion:
    """`region` on a model (3D doc §4).

    Attributes:
        input_ (RegionInput): How a model receives a `region`.
        description (Union[None, Unset, str]):
        label (Union[None, Unset, str]):
        max_aspect (Union[None, Unset, float]): The vendor's largest box aspect ratio (6 for 1:6–6:1).
        min_window_px (Union[None, Unset, int]): The vendor's smallest box side, in pixels. A smaller region gets a
            window enlarged to it, with the exact region sent as the mask.
    """

    input_: RegionInput
    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_aspect: Union[None, Unset, float] = UNSET
    min_window_px: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        input_ = self.input_.value

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

        max_aspect: Union[None, Unset, float]
        if isinstance(self.max_aspect, Unset):
            max_aspect = UNSET
        else:
            max_aspect = self.max_aspect

        min_window_px: Union[None, Unset, int]
        if isinstance(self.min_window_px, Unset):
            min_window_px = UNSET
        else:
            min_window_px = self.min_window_px

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "input": input_,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if max_aspect is not UNSET:
            field_dict["max_aspect"] = max_aspect
        if min_window_px is not UNSET:
            field_dict["min_window_px"] = min_window_px

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        input_ = RegionInput(d.pop("input"))

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

        def _parse_max_aspect(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        max_aspect = _parse_max_aspect(d.pop("max_aspect", UNSET))

        def _parse_min_window_px(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        min_window_px = _parse_min_window_px(d.pop("min_window_px", UNSET))

        param_spec_region = cls(
            input_=input_,
            description=description,
            label=label,
            max_aspect=max_aspect,
            min_window_px=min_window_px,
        )

        param_spec_region.additional_properties = d
        return param_spec_region

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
