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

from ..models.model_3d_format import Model3DFormat
from ..models.source_kind import SourceKind
from ..types import UNSET, Unset

T = TypeVar("T", bound="SourceRef")


@_attrs_define
class SourceRef:
    """Where an operation's input comes from: a stored litegen generation, or a
    file of your own.

        Attributes:
            type (SourceKind): How a `SourceRef`'s `value` is to be read.
            value (str): The generation id, the URL, or the base64 bytes (a `data:` URI prefix
                is allowed).
            format_ (Union[Model3DFormat, None, Unset]):
    """

    type: SourceKind
    value: str
    format_: Union[Model3DFormat, None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        type = self.type.value

        value = self.value

        format_: Union[None, Unset, str]
        if isinstance(self.format_, Unset):
            format_ = UNSET
        elif isinstance(self.format_, Model3DFormat):
            format_ = self.format_.value
        else:
            format_ = self.format_

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "type": type,
                "value": value,
            }
        )
        if format_ is not UNSET:
            field_dict["format"] = format_

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        type = SourceKind(d.pop("type"))

        value = d.pop("value")

        def _parse_format_(data: object) -> Union[Model3DFormat, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                format_type_1 = Model3DFormat(data)

                return format_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DFormat, None, Unset], data)

        format_ = _parse_format_(d.pop("format", UNSET))

        source_ref = cls(
            type=type,
            value=value,
            format_=format_,
        )

        source_ref.additional_properties = d
        return source_ref

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
