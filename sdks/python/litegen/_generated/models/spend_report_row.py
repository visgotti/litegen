from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="SpendReportRow")


@_attrs_define
class SpendReportRow:
    """One group of a spend report.

    Attributes:
        cost_usd (float):
        failed_requests (int):
        requests (int):
        group (Union[None, Unset, str]): The group value (a day `YYYY-MM-DD`, model id, key id, …). Null when the
            requests carried no value for the dimension (e.g. no end user).
    """

    cost_usd: float
    failed_requests: int
    requests: int
    group: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cost_usd = self.cost_usd

        failed_requests = self.failed_requests

        requests = self.requests

        group: Union[None, Unset, str]
        if isinstance(self.group, Unset):
            group = UNSET
        else:
            group = self.group

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cost_usd": cost_usd,
                "failed_requests": failed_requests,
                "requests": requests,
            }
        )
        if group is not UNSET:
            field_dict["group"] = group

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        cost_usd = d.pop("cost_usd")

        failed_requests = d.pop("failed_requests")

        requests = d.pop("requests")

        def _parse_group(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        group = _parse_group(d.pop("group", UNSET))

        spend_report_row = cls(
            cost_usd=cost_usd,
            failed_requests=failed_requests,
            requests=requests,
            group=group,
        )

        spend_report_row.additional_properties = d
        return spend_report_row

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
