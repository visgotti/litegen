from typing import Any, Dict, List, Type, TypeVar, Union

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="Deployment")


@_attrs_define
class Deployment:
    """A single deployment in a routing chain.

    Attributes:
        provider (str): Provider config name/id.
        max_retries (Union[Unset, int]): Max retries before falling to next deployment.
        respect_health (Union[Unset, bool]): Whether to skip this deployment on health check failure.
        rpm_limit (Union[Unset, int]): Rate limit (requests per minute). 0 = unlimited.
        timeout_seconds (Union[Unset, int]): Timeout in seconds.
        weight (Union[Unset, int]): Weight for weighted routing.
    """

    provider: str
    max_retries: Union[Unset, int] = UNSET
    respect_health: Union[Unset, bool] = UNSET
    rpm_limit: Union[Unset, int] = UNSET
    timeout_seconds: Union[Unset, int] = UNSET
    weight: Union[Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        provider = self.provider

        max_retries = self.max_retries

        respect_health = self.respect_health

        rpm_limit = self.rpm_limit

        timeout_seconds = self.timeout_seconds

        weight = self.weight

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "provider": provider,
            }
        )
        if max_retries is not UNSET:
            field_dict["max_retries"] = max_retries
        if respect_health is not UNSET:
            field_dict["respect_health"] = respect_health
        if rpm_limit is not UNSET:
            field_dict["rpm_limit"] = rpm_limit
        if timeout_seconds is not UNSET:
            field_dict["timeout_seconds"] = timeout_seconds
        if weight is not UNSET:
            field_dict["weight"] = weight

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        provider = d.pop("provider")

        max_retries = d.pop("max_retries", UNSET)

        respect_health = d.pop("respect_health", UNSET)

        rpm_limit = d.pop("rpm_limit", UNSET)

        timeout_seconds = d.pop("timeout_seconds", UNSET)

        weight = d.pop("weight", UNSET)

        deployment = cls(
            provider=provider,
            max_retries=max_retries,
            respect_health=respect_health,
            rpm_limit=rpm_limit,
            timeout_seconds=timeout_seconds,
            weight=weight,
        )

        deployment.additional_properties = d
        return deployment

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
