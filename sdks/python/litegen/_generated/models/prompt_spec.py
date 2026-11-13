from typing import Any, Dict, Type, TypeVar, Union, cast

from attrs import define as _attrs_define

from ..types import UNSET, Unset

T = TypeVar("T", bound="PromptSpec")


@_attrs_define
class PromptSpec:
    """How a model treats the request's `prompt`. Lengths count characters.

    Attributes:
        max_length (Union[None, Unset, int]): `0` means the model takes no prompt at all (an image-only endpoint
            whose vendor has no prompt field): a client should not offer one, and a
            non-empty prompt is refused in strict mode and dropped in lax mode.
        min_length (Union[None, Unset, int]):
        required (Union[Unset, bool]): An empty or omitted prompt is refused with `prompt_required`. 3D models
            that take images set this false, yet still need a prompt when a request
            carries no image (text-to-3D).
        required_for_text_to_video (Union[Unset, bool]): A video model whose text-to-video mode needs the prompt though
            its
            image-to-video mode does not (so `required` is false): a request with
            no `first_frame` or `init` image and an empty prompt is refused with
            `prompt_required` before anything is quoted or reserved. Absent
            (false): an empty text-to-video request goes to the vendor, as for a
            vendor whose shots can carry the prompts.
    """

    max_length: Union[None, Unset, int] = UNSET
    min_length: Union[None, Unset, int] = UNSET
    required: Union[Unset, bool] = UNSET
    required_for_text_to_video: Union[Unset, bool] = UNSET

    def to_dict(self) -> Dict[str, Any]:
        max_length: Union[None, Unset, int]
        if isinstance(self.max_length, Unset):
            max_length = UNSET
        else:
            max_length = self.max_length

        min_length: Union[None, Unset, int]
        if isinstance(self.min_length, Unset):
            min_length = UNSET
        else:
            min_length = self.min_length

        required = self.required

        required_for_text_to_video = self.required_for_text_to_video

        field_dict: Dict[str, Any] = {}
        field_dict.update({})
        if max_length is not UNSET:
            field_dict["max_length"] = max_length
        if min_length is not UNSET:
            field_dict["min_length"] = min_length
        if required is not UNSET:
            field_dict["required"] = required
        if required_for_text_to_video is not UNSET:
            field_dict["required_for_text_to_video"] = required_for_text_to_video

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_max_length(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_length = _parse_max_length(d.pop("max_length", UNSET))

        def _parse_min_length(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        min_length = _parse_min_length(d.pop("min_length", UNSET))

        required = d.pop("required", UNSET)

        required_for_text_to_video = d.pop("required_for_text_to_video", UNSET)

        prompt_spec = cls(
            max_length=max_length,
            min_length=min_length,
            required=required,
            required_for_text_to_video=required_for_text_to_video,
        )

        return prompt_spec
