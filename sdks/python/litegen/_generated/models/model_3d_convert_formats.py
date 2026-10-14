from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.model_3d_format import Model3DFormat

T = TypeVar("T", bound="Model3DConvertFormats")


@_attrs_define
class Model3DConvertFormats:
    """What the converter can do, for a client to check before uploading.

    Attributes:
        formats (List[Model3DFormat]): Containers accepted as both `from` and `to`. Every pair converts, in
            both directions. Grows with litegen's converter, so read it rather than
            hardcoding it; it can be narrower than the `Model3dFormat` vocabulary.
        max_input_bytes (int): Largest request body accepted, in bytes. Larger bodies get `413`.
    """

    formats: List[Model3DFormat]
    max_input_bytes: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        formats = []
        for formats_item_data in self.formats:
            formats_item = formats_item_data.value
            formats.append(formats_item)

        max_input_bytes = self.max_input_bytes

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "formats": formats,
                "max_input_bytes": max_input_bytes,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        formats = []
        _formats = d.pop("formats")
        for formats_item_data in _formats:
            formats_item = Model3DFormat(formats_item_data)

            formats.append(formats_item)

        max_input_bytes = d.pop("max_input_bytes")

        model_3d_convert_formats = cls(
            formats=formats,
            max_input_bytes=max_input_bytes,
        )

        model_3d_convert_formats.additional_properties = d
        return model_3d_convert_formats

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
