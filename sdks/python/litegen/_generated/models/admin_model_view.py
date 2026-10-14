from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.model_source import ModelSource

if TYPE_CHECKING:
    from ..models.model_schema import ModelSchema


T = TypeVar("T", bound="AdminModelView")


@_attrs_define
class AdminModelView:
    """Admin view of one model: its full schema plus management state.

    Attributes:
        disabled (bool): Disabled models are hidden from `/v1/models` and refused at generation.
        id (str):
        schema (ModelSchema):
        source (ModelSource): Where a served model definition comes from.
    """

    disabled: bool
    id: str
    schema: "ModelSchema"
    source: ModelSource
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        disabled = self.disabled

        id = self.id

        schema = self.schema.to_dict()

        source = self.source.value

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "disabled": disabled,
                "id": id,
                "schema": schema,
                "source": source,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.model_schema import ModelSchema

        d = src_dict.copy()
        disabled = d.pop("disabled")

        id = d.pop("id")

        schema = ModelSchema.from_dict(d.pop("schema"))

        source = ModelSource(d.pop("source"))

        admin_model_view = cls(
            disabled=disabled,
            id=id,
            schema=schema,
            source=source,
        )

        admin_model_view.additional_properties = d
        return admin_model_view

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
