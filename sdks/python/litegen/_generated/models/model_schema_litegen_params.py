from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="ModelSchemaLitegenParams")


@_attrs_define
class ModelSchemaLitegenParams:
    """Request fields litegen applies to this model itself, beyond what its
    vendor takes, each with the bounds litegen holds it to and in the same
    `ParamSpec` shape as `params`: on every 3D model `ground`, `center` and
    `height_m` (unless `params` declares the model's own `height_m`); on an
    image model that takes a `mask` reference or a `region`,
    `mask_grow_px`, `preserve_unmasked` and `feather_px`. No name is in
    both maps. Computed by the server and served by `GET /v1/models/{id}`,
    so read-only (`readOnly` in the OpenAPI document): a catalog row that
    sets it is refused, and the admin API reads and ignores it, as it
    does `status`, so a served schema posted back to `POST`/`PUT
    /v1/admin/models` works.

    """

    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        model_schema_litegen_params = cls()

        model_schema_litegen_params.additional_properties = d
        return model_schema_litegen_params

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
