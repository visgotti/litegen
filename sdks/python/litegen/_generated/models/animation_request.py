from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="AnimationRequest")


@_attrs_define
class AnimationRequest:
    """One clip an `animate` request asks for (3D doc §3C).

    Attributes:
        name (str): The clip's name in the delivered file: unique within the request, 1–64
            characters of `[A-Za-z0-9_.-]`.
        duration_s (Union[None, Unset, float]): A prompt clip's length in seconds, within the model's
            `text_to_motion` range.
        in_place (Union[None, Unset, bool]): Keep the root in place horizontally (vertical movement stays).
        loop (Union[None, Unset, bool]): A loop hint, recorded in the manifest; nothing is resampled to force a
            loop.
        preset (Union[None, Unset, str]): A canonical preset (`Model3dAnimationPreset`) or one of the model's
            own preset ids. Exactly one of `preset` and `prompt`.
        prompt (Union[None, Unset, str]): Text-to-motion, where the model offers it.
    """

    name: str
    duration_s: Union[None, Unset, float] = UNSET
    in_place: Union[None, Unset, bool] = UNSET
    loop: Union[None, Unset, bool] = UNSET
    preset: Union[None, Unset, str] = UNSET
    prompt: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        name = self.name

        duration_s: Union[None, Unset, float]
        if isinstance(self.duration_s, Unset):
            duration_s = UNSET
        else:
            duration_s = self.duration_s

        in_place: Union[None, Unset, bool]
        if isinstance(self.in_place, Unset):
            in_place = UNSET
        else:
            in_place = self.in_place

        loop: Union[None, Unset, bool]
        if isinstance(self.loop, Unset):
            loop = UNSET
        else:
            loop = self.loop

        preset: Union[None, Unset, str]
        if isinstance(self.preset, Unset):
            preset = UNSET
        else:
            preset = self.preset

        prompt: Union[None, Unset, str]
        if isinstance(self.prompt, Unset):
            prompt = UNSET
        else:
            prompt = self.prompt

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "name": name,
            }
        )
        if duration_s is not UNSET:
            field_dict["duration_s"] = duration_s
        if in_place is not UNSET:
            field_dict["in_place"] = in_place
        if loop is not UNSET:
            field_dict["loop"] = loop
        if preset is not UNSET:
            field_dict["preset"] = preset
        if prompt is not UNSET:
            field_dict["prompt"] = prompt

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        name = d.pop("name")

        def _parse_duration_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        duration_s = _parse_duration_s(d.pop("duration_s", UNSET))

        def _parse_in_place(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        in_place = _parse_in_place(d.pop("in_place", UNSET))

        def _parse_loop(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        loop = _parse_loop(d.pop("loop", UNSET))

        def _parse_preset(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        preset = _parse_preset(d.pop("preset", UNSET))

        def _parse_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        prompt = _parse_prompt(d.pop("prompt", UNSET))

        animation_request = cls(
            name=name,
            duration_s=duration_s,
            in_place=in_place,
            loop=loop,
            preset=preset,
            prompt=prompt,
        )

        animation_request.additional_properties = d
        return animation_request

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
