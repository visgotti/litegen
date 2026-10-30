from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.generation_asset import GenerationAsset


T = TypeVar("T", bound="GenerationAssetList")


@_attrs_define
class GenerationAssetList:
    """`GET /v1/generations/{id}/assets`.

    Attributes:
        data (List['GenerationAsset']):
        generation_id (str):
    """

    data: List["GenerationAsset"]
    generation_id: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        data = []
        for data_item_data in self.data:
            data_item = data_item_data.to_dict()
            data.append(data_item)

        generation_id = self.generation_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "generation_id": generation_id,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.generation_asset import GenerationAsset

        d = src_dict.copy()
        data = []
        _data = d.pop("data")
        for data_item_data in _data:
            data_item = GenerationAsset.from_dict(data_item_data)

            data.append(data_item)

        generation_id = d.pop("generation_id")

        generation_asset_list = cls(
            data=data,
            generation_id=generation_id,
        )

        generation_asset_list.additional_properties = d
        return generation_asset_list

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
