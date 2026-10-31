from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.keyframe_timing import KeyframeTiming
from ..models.mask_convention import MaskConvention
from ..types import UNSET, Unset

T = TypeVar("T", bound="RefRoleSpec")


@_attrs_define
class RefRoleSpec:
    """
    Attributes:
        max_count (int):
        min_count (int):
        convention (Union[MaskConvention, None, Unset]):
        max_time_s (Union[None, Unset, float]): The latest a `keyframe` may sit, for a model with no duration param
            to bound its clip: the longest clip it makes or prices.
        native_grow (Union[Unset, bool]): The mask role's vendor grows masks itself (Stability's `grow_mask`):
            `mask_grow_px` goes to it, explicitly, instead of litegen dilating.
        required (Union[Unset, bool]):
        time_step_s (Union[None, Unset, float]): The `keyframe` role's time grid, when the vendor takes only multiples
            of it (PixVerse: whole seconds). A time off it is refused.
        timing (Union[KeyframeTiming, None, Unset]):
    """

    max_count: int
    min_count: int
    convention: Union[MaskConvention, None, Unset] = UNSET
    max_time_s: Union[None, Unset, float] = UNSET
    native_grow: Union[Unset, bool] = UNSET
    required: Union[Unset, bool] = UNSET
    time_step_s: Union[None, Unset, float] = UNSET
    timing: Union[KeyframeTiming, None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_count = self.max_count

        min_count = self.min_count

        convention: Union[None, Unset, str]
        if isinstance(self.convention, Unset):
            convention = UNSET
        elif isinstance(self.convention, MaskConvention):
            convention = self.convention.value
        else:
            convention = self.convention

        max_time_s: Union[None, Unset, float]
        if isinstance(self.max_time_s, Unset):
            max_time_s = UNSET
        else:
            max_time_s = self.max_time_s

        native_grow = self.native_grow

        required = self.required

        time_step_s: Union[None, Unset, float]
        if isinstance(self.time_step_s, Unset):
            time_step_s = UNSET
        else:
            time_step_s = self.time_step_s

        timing: Union[None, Unset, str]
        if isinstance(self.timing, Unset):
            timing = UNSET
        elif isinstance(self.timing, KeyframeTiming):
            timing = self.timing.value
        else:
            timing = self.timing

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_count": max_count,
                "min_count": min_count,
            }
        )
        if convention is not UNSET:
            field_dict["convention"] = convention
        if max_time_s is not UNSET:
            field_dict["max_time_s"] = max_time_s
        if native_grow is not UNSET:
            field_dict["native_grow"] = native_grow
        if required is not UNSET:
            field_dict["required"] = required
        if time_step_s is not UNSET:
            field_dict["time_step_s"] = time_step_s
        if timing is not UNSET:
            field_dict["timing"] = timing

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        max_count = d.pop("max_count")

        min_count = d.pop("min_count")

        def _parse_convention(data: object) -> Union[MaskConvention, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                convention_type_1 = MaskConvention(data)

                return convention_type_1
            except:  # noqa: E722
                pass
            return cast(Union[MaskConvention, None, Unset], data)

        convention = _parse_convention(d.pop("convention", UNSET))

        def _parse_max_time_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        max_time_s = _parse_max_time_s(d.pop("max_time_s", UNSET))

        native_grow = d.pop("native_grow", UNSET)

        required = d.pop("required", UNSET)

        def _parse_time_step_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        time_step_s = _parse_time_step_s(d.pop("time_step_s", UNSET))

        def _parse_timing(data: object) -> Union[KeyframeTiming, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                timing_type_1 = KeyframeTiming(data)

                return timing_type_1
            except:  # noqa: E722
                pass
            return cast(Union[KeyframeTiming, None, Unset], data)

        timing = _parse_timing(d.pop("timing", UNSET))

        ref_role_spec = cls(
            max_count=max_count,
            min_count=min_count,
            convention=convention,
            max_time_s=max_time_s,
            native_grow=native_grow,
            required=required,
            time_step_s=time_step_s,
            timing=timing,
        )

        ref_role_spec.additional_properties = d
        return ref_role_spec

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
