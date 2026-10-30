from typing import (
    TYPE_CHECKING,
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

from ..models.in_place_mode import InPlaceMode
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.animation_preset_spec import AnimationPresetSpec
    from ..models.text_to_motion_spec import TextToMotionSpec


T = TypeVar("T", bound="ParamSpecAnimationList")


@_attrs_define
class ParamSpecAnimationList:
    """The clips an `animate` model makes (`animations`, 3D doc §3C).

    Attributes:
        max_items (int): The most clips one request may ask for.
        description (Union[None, Unset, str]):
        in_place (Union[Unset, InPlaceMode]): Who keeps a clip in place.
        label (Union[None, Unset, str]):
        presets (Union[Unset, List['AnimationPresetSpec']]): The model's presets: its own ids, each optionally tagged
            with the
            canonical preset (`Model3dAnimationPreset`) it plays. A request names
            either.
        text_to_motion (Union['TextToMotionSpec', None, Unset]):
    """

    max_items: int
    description: Union[None, Unset, str] = UNSET
    in_place: Union[Unset, InPlaceMode] = UNSET
    label: Union[None, Unset, str] = UNSET
    presets: Union[Unset, List["AnimationPresetSpec"]] = UNSET
    text_to_motion: Union["TextToMotionSpec", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.text_to_motion_spec import TextToMotionSpec

        max_items = self.max_items

        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        in_place: Union[Unset, str] = UNSET
        if not isinstance(self.in_place, Unset):
            in_place = self.in_place.value

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        presets: Union[Unset, List[Dict[str, Any]]] = UNSET
        if not isinstance(self.presets, Unset):
            presets = []
            for presets_item_data in self.presets:
                presets_item = presets_item_data.to_dict()
                presets.append(presets_item)

        text_to_motion: Union[Dict[str, Any], None, Unset]
        if isinstance(self.text_to_motion, Unset):
            text_to_motion = UNSET
        elif isinstance(self.text_to_motion, TextToMotionSpec):
            text_to_motion = self.text_to_motion.to_dict()
        else:
            text_to_motion = self.text_to_motion

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_items": max_items,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if in_place is not UNSET:
            field_dict["in_place"] = in_place
        if label is not UNSET:
            field_dict["label"] = label
        if presets is not UNSET:
            field_dict["presets"] = presets
        if text_to_motion is not UNSET:
            field_dict["text_to_motion"] = text_to_motion

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.animation_preset_spec import AnimationPresetSpec
        from ..models.text_to_motion_spec import TextToMotionSpec

        d = src_dict.copy()
        max_items = d.pop("max_items")

        def _parse_description(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        description = _parse_description(d.pop("description", UNSET))

        _in_place = d.pop("in_place", UNSET)
        in_place: Union[Unset, InPlaceMode]
        if isinstance(_in_place, Unset):
            in_place = UNSET
        else:
            in_place = InPlaceMode(_in_place)

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        presets = []
        _presets = d.pop("presets", UNSET)
        for presets_item_data in _presets or []:
            presets_item = AnimationPresetSpec.from_dict(presets_item_data)

            presets.append(presets_item)

        def _parse_text_to_motion(
            data: object,
        ) -> Union["TextToMotionSpec", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                text_to_motion_type_1 = TextToMotionSpec.from_dict(data)

                return text_to_motion_type_1
            except:  # noqa: E722
                pass
            return cast(Union["TextToMotionSpec", None, Unset], data)

        text_to_motion = _parse_text_to_motion(d.pop("text_to_motion", UNSET))

        param_spec_animation_list = cls(
            max_items=max_items,
            description=description,
            in_place=in_place,
            label=label,
            presets=presets,
            text_to_motion=text_to_motion,
        )

        param_spec_animation_list.additional_properties = d
        return param_spec_animation_list

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
