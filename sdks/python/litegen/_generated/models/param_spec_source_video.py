from typing import (
    Any,
    Dict,
    List,
    Type,
    TypeVar,
    Union,
    cast,
)

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.extend_output import ExtendOutput
from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecSourceVideo")


@_attrs_define
class ParamSpecSourceVideo:
    """`source_video` on a video operation (3D doc §5): the clips it works on.

    Attributes:
        description (Union[None, Unset, str]):
        extend_output (Union[ExtendOutput, None, Unset]):
        formats (Union[Unset, List[str]]): Containers the vendor accepts: `mp4`, `mov`, `webm`, `gif`.
        label (Union[None, Unset, str]):
        max_duration_s (Union[None, Unset, float]): The longest source it takes, in seconds.
        own_generations_only (Union[Unset, bool]): Only the vendor's own earlier generations, on the same account (Luma,
            Kling): any other source is `source_not_chainable`.
    """

    description: Union[None, Unset, str] = UNSET
    extend_output: Union[ExtendOutput, None, Unset] = UNSET
    formats: Union[Unset, List[str]] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_duration_s: Union[None, Unset, float] = UNSET
    own_generations_only: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        extend_output: Union[None, Unset, str]
        if isinstance(self.extend_output, Unset):
            extend_output = UNSET
        elif isinstance(self.extend_output, ExtendOutput):
            extend_output = self.extend_output.value
        else:
            extend_output = self.extend_output

        formats: Union[Unset, List[str]] = UNSET
        if not isinstance(self.formats, Unset):
            formats = self.formats

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        max_duration_s: Union[None, Unset, float]
        if isinstance(self.max_duration_s, Unset):
            max_duration_s = UNSET
        else:
            max_duration_s = self.max_duration_s

        own_generations_only = self.own_generations_only

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if description is not UNSET:
            field_dict["description"] = description
        if extend_output is not UNSET:
            field_dict["extend_output"] = extend_output
        if formats is not UNSET:
            field_dict["formats"] = formats
        if label is not UNSET:
            field_dict["label"] = label
        if max_duration_s is not UNSET:
            field_dict["max_duration_s"] = max_duration_s
        if own_generations_only is not UNSET:
            field_dict["own_generations_only"] = own_generations_only

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

        def _parse_extend_output(data: object) -> Union[ExtendOutput, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                extend_output_type_1 = ExtendOutput(data)

                return extend_output_type_1
            except:  # noqa: E722
                pass
            return cast(Union[ExtendOutput, None, Unset], data)

        extend_output = _parse_extend_output(d.pop("extend_output", UNSET))

        formats = cast(List[str], d.pop("formats", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_max_duration_s(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        max_duration_s = _parse_max_duration_s(d.pop("max_duration_s", UNSET))

        own_generations_only = d.pop("own_generations_only", UNSET)

        param_spec_source_video = cls(
            description=description,
            extend_output=extend_output,
            formats=formats,
            label=label,
            max_duration_s=max_duration_s,
            own_generations_only=own_generations_only,
        )

        param_spec_source_video.additional_properties = d
        return param_spec_source_video

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
