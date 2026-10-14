from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.spend_log_entry import SpendLogEntry


T = TypeVar("T", bound="SpendLogsResponse")


@_attrs_define
class SpendLogsResponse:
    """Response for `GET /v1/spend/logs`.

    Attributes:
        data (List['SpendLogEntry']):
        page (int):
        per_page (int):
        total (int):
    """

    data: List["SpendLogEntry"]
    page: int
    per_page: int
    total: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        data = []
        for data_item_data in self.data:
            data_item = data_item_data.to_dict()
            data.append(data_item)

        page = self.page

        per_page = self.per_page

        total = self.total

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "page": page,
                "per_page": per_page,
                "total": total,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.spend_log_entry import SpendLogEntry

        d = src_dict.copy()
        data = []
        _data = d.pop("data")
        for data_item_data in _data:
            data_item = SpendLogEntry.from_dict(data_item_data)

            data.append(data_item)

        page = d.pop("page")

        per_page = d.pop("per_page")

        total = d.pop("total")

        spend_logs_response = cls(
            data=data,
            page=page,
            per_page=per_page,
            total=total,
        )

        spend_logs_response.additional_properties = d
        return spend_logs_response

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
