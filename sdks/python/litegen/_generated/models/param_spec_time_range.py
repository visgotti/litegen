from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecTimeRange")


@_attrs_define
class ParamSpecTimeRange:
    """`time_range` on a video operation (3D doc §5): the windows it takes.

    Attributes:
        description (Union[None, Unset, str]):
        label (Union[None, Unset, str]):
        min_window_s (Union[None, Unset, float]): The shortest window, in seconds (LTX's 2 s).
        whole_seconds (Union[Unset, bool]): Starts and ends fall on whole seconds (Runway).
    """

    description: Union[None, Unset, str] = UNSET
    label: Union[None, Unset, str] = UNSET
    min_window_s: Union[None, Unset, float] = UNSET
    whole_seconds: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
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

        min_window_s: Union[None, Unset, float]
        if isinstance(self.min_window_s, Unset):
            min_window_s = UNSET
        else:
            min_window_s = self.min_window_s

        whole_seconds = self.whole_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if description is not UNSET:
            field_dict["description"] = description
        if label is not UNSET:
            field_dict["label"] = label
        if min_window_s is not UNSET:
            field_dict["min_window_s"] = min_window_s
        if whole_seconds is not UNSET:
            field_dict["whole_seconds"] = whole_seconds

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

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

        def _parse_min_window_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        min_window_s = _parse_min_window_s(d.pop("min_window_s", UNSET))

        whole_seconds = d.pop("whole_seconds", UNSET)

        param_spec_time_range = cls(
            description=description,
            label=label,
            min_window_s=min_window_s,
            whole_seconds=whole_seconds,
        )

        param_spec_time_range.additional_properties = d
        return param_spec_time_range

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
