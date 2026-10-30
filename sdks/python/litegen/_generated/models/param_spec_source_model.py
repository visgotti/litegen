from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ParamSpecSourceModel")


@_attrs_define
class ParamSpecSourceModel:
    """What an operation on an existing mesh takes as its `source_model` (3D doc
    §3A). litegen reads the source with its own mesh reader and refuses one the
    vendor would refuse — before the billed submit — then converts it to a
    container in `formats` if it is not already one.

        Attributes:
            description (Union[None, Unset, str]):
            formats (Union[Unset, List[str]]): Containers the vendor accepts, from the `Model3dFormat` vocabulary. A
                source in any other container litegen can read is converted to the
                first of these it can write.
            label (Union[None, Unset, str]):
            max_bytes (Union[None, Unset, int]): Largest file the vendor accepts, in bytes, as sent.
            max_faces (Union[None, Unset, int]): Most triangles the vendor accepts.
            requires_skin (Union[Unset, bool]): The source must already be rigged (have a skin).
            requires_texture (Union[Unset, bool]): The source must be textured.
    """

    description: Union[None, Unset, str] = UNSET
    formats: Union[Unset, List[str]] = UNSET
    label: Union[None, Unset, str] = UNSET
    max_bytes: Union[None, Unset, int] = UNSET
    max_faces: Union[None, Unset, int] = UNSET
    requires_skin: Union[Unset, bool] = UNSET
    requires_texture: Union[Unset, bool] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        description: Union[None, Unset, str]
        if isinstance(self.description, Unset):
            description = UNSET
        else:
            description = self.description

        formats: Union[Unset, List[str]] = UNSET
        if not isinstance(self.formats, Unset):
            formats = self.formats

        label: Union[None, Unset, str]
        if isinstance(self.label, Unset):
            label = UNSET
        else:
            label = self.label

        max_bytes: Union[None, Unset, int]
        if isinstance(self.max_bytes, Unset):
            max_bytes = UNSET
        else:
            max_bytes = self.max_bytes

        max_faces: Union[None, Unset, int]
        if isinstance(self.max_faces, Unset):
            max_faces = UNSET
        else:
            max_faces = self.max_faces

        requires_skin = self.requires_skin

        requires_texture = self.requires_texture

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if description is not UNSET:
            field_dict["description"] = description
        if formats is not UNSET:
            field_dict["formats"] = formats
        if label is not UNSET:
            field_dict["label"] = label
        if max_bytes is not UNSET:
            field_dict["max_bytes"] = max_bytes
        if max_faces is not UNSET:
            field_dict["max_faces"] = max_faces
        if requires_skin is not UNSET:
            field_dict["requires_skin"] = requires_skin
        if requires_texture is not UNSET:
            field_dict["requires_texture"] = requires_texture

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

        formats = cast(List[str], d.pop("formats", UNSET))

        def _parse_label(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        label = _parse_label(d.pop("label", UNSET))

        def _parse_max_bytes(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_bytes = _parse_max_bytes(d.pop("max_bytes", UNSET))

        def _parse_max_faces(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_faces = _parse_max_faces(d.pop("max_faces", UNSET))

        requires_skin = d.pop("requires_skin", UNSET)

        requires_texture = d.pop("requires_texture", UNSET)

        param_spec_source_model = cls(
            description=description,
            formats=formats,
            label=label,
            max_bytes=max_bytes,
            max_faces=max_faces,
            requires_skin=requires_skin,
            requires_texture=requires_texture,
        )

        param_spec_source_model.additional_properties = d
        return param_spec_source_model

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
