from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.readiness_checks import ReadinessChecks


T = TypeVar("T", bound="ReadinessResponse")


@_attrs_define
class ReadinessResponse:
    """Response for `GET /health/ready`.

    Attributes:
        checks (ReadinessChecks):
        status (str): "ready" or "not_ready"
    """

    checks: "ReadinessChecks"
    status: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        checks = self.checks.to_dict()

        status = self.status

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "checks": checks,
                "status": status,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.readiness_checks import ReadinessChecks

        d = src_dict.copy()
        checks = ReadinessChecks.from_dict(d.pop("checks"))

        status = d.pop("status")

        readiness_response = cls(
            checks=checks,
            status=status,
        )

        readiness_response.additional_properties = d
        return readiness_response

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
