import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.spend_group_by import SpendGroupBy
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.spend_report_row import SpendReportRow


T = TypeVar("T", bound="SpendReportResponse")


@_attrs_define
class SpendReportResponse:
    """Response for `GET /v1/spend/report`.

    Attributes:
        data (List['SpendReportRow']):
        group_by (SpendGroupBy): Dimension a spend report is grouped by.
        total_cost_usd (float):
        total_requests (int):
        from_ (Union[None, Unset, datetime.datetime]):
        to (Union[None, Unset, datetime.datetime]):
    """

    data: List["SpendReportRow"]
    group_by: SpendGroupBy
    total_cost_usd: float
    total_requests: int
    from_: Union[None, Unset, datetime.datetime] = UNSET
    to: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        data = []
        for data_item_data in self.data:
            data_item = data_item_data.to_dict()
            data.append(data_item)

        group_by = self.group_by.value

        total_cost_usd = self.total_cost_usd

        total_requests = self.total_requests

        from_: Union[None, Unset, str]
        if isinstance(self.from_, Unset):
            from_ = UNSET
        elif isinstance(self.from_, datetime.datetime):
            from_ = self.from_.isoformat()
        else:
            from_ = self.from_

        to: Union[None, Unset, str]
        if isinstance(self.to, Unset):
            to = UNSET
        elif isinstance(self.to, datetime.datetime):
            to = self.to.isoformat()
        else:
            to = self.to

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "data": data,
                "group_by": group_by,
                "total_cost_usd": total_cost_usd,
                "total_requests": total_requests,
            }
        )
        if from_ is not UNSET:
            field_dict["from"] = from_
        if to is not UNSET:
            field_dict["to"] = to

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.spend_report_row import SpendReportRow

        d = src_dict.copy()
        data = []
        _data = d.pop("data")
        for data_item_data in _data:
            data_item = SpendReportRow.from_dict(data_item_data)

            data.append(data_item)

        group_by = SpendGroupBy(d.pop("group_by"))

        total_cost_usd = d.pop("total_cost_usd")

        total_requests = d.pop("total_requests")

        def _parse_from_(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                from_type_0 = isoparse(data)

                return from_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        from_ = _parse_from_(d.pop("from", UNSET))

        def _parse_to(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                to_type_0 = isoparse(data)

                return to_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        to = _parse_to(d.pop("to", UNSET))

        spend_report_response = cls(
            data=data,
            group_by=group_by,
            total_cost_usd=total_cost_usd,
            total_requests=total_requests,
            from_=from_,
            to=to,
        )

        spend_report_response.additional_properties = d
        return spend_report_response

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
