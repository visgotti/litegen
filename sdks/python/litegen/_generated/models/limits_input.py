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

from ..types import UNSET, Unset

T = TypeVar("T", bound="LimitsInput")


@_attrs_define
class LimitsInput:
    """Request body for `PUT …/limits`. A full replacement of the configurable
    fields: an omitted field is cleared. Accumulated spend is kept unless the
    budget window changes, which starts a fresh window from now.

        Attributes:
            allowed_models (Union[List[str], None, Unset]):
            budget_duration (Union[None, Unset, str]):
            max_budget_usd (Union[None, Unset, float]):
            max_parallel_requests (Union[None, Unset, int]):
            metadata (Union[Unset, Any]):
            rpm_limit (Union[None, Unset, int]):
            soft_budget_usd (Union[None, Unset, float]):
    """

    allowed_models: Union[List[str], None, Unset] = UNSET
    budget_duration: Union[None, Unset, str] = UNSET
    max_budget_usd: Union[None, Unset, float] = UNSET
    max_parallel_requests: Union[None, Unset, int] = UNSET
    metadata: Union[Unset, Any] = UNSET
    rpm_limit: Union[None, Unset, int] = UNSET
    soft_budget_usd: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        allowed_models: Union[List[str], None, Unset]
        if isinstance(self.allowed_models, Unset):
            allowed_models = UNSET
        elif isinstance(self.allowed_models, list):
            allowed_models = self.allowed_models

        else:
            allowed_models = self.allowed_models

        budget_duration: Union[None, Unset, str]
        if isinstance(self.budget_duration, Unset):
            budget_duration = UNSET
        else:
            budget_duration = self.budget_duration

        max_budget_usd: Union[None, Unset, float]
        if isinstance(self.max_budget_usd, Unset):
            max_budget_usd = UNSET
        else:
            max_budget_usd = self.max_budget_usd

        max_parallel_requests: Union[None, Unset, int]
        if isinstance(self.max_parallel_requests, Unset):
            max_parallel_requests = UNSET
        else:
            max_parallel_requests = self.max_parallel_requests

        metadata = self.metadata

        rpm_limit: Union[None, Unset, int]
        if isinstance(self.rpm_limit, Unset):
            rpm_limit = UNSET
        else:
            rpm_limit = self.rpm_limit

        soft_budget_usd: Union[None, Unset, float]
        if isinstance(self.soft_budget_usd, Unset):
            soft_budget_usd = UNSET
        else:
            soft_budget_usd = self.soft_budget_usd

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if allowed_models is not UNSET:
            field_dict["allowed_models"] = allowed_models
        if budget_duration is not UNSET:
            field_dict["budget_duration"] = budget_duration
        if max_budget_usd is not UNSET:
            field_dict["max_budget_usd"] = max_budget_usd
        if max_parallel_requests is not UNSET:
            field_dict["max_parallel_requests"] = max_parallel_requests
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if rpm_limit is not UNSET:
            field_dict["rpm_limit"] = rpm_limit
        if soft_budget_usd is not UNSET:
            field_dict["soft_budget_usd"] = soft_budget_usd

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_allowed_models(data: object) -> Union[List[str], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                allowed_models_type_0 = cast(List[str], data)

                return allowed_models_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List[str], None, Unset], data)

        allowed_models = _parse_allowed_models(d.pop("allowed_models", UNSET))

        def _parse_budget_duration(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        budget_duration = _parse_budget_duration(d.pop("budget_duration", UNSET))

        def _parse_max_budget_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        max_budget_usd = _parse_max_budget_usd(d.pop("max_budget_usd", UNSET))

        def _parse_max_parallel_requests(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        max_parallel_requests = _parse_max_parallel_requests(
            d.pop("max_parallel_requests", UNSET)
        )

        metadata = d.pop("metadata", UNSET)

        def _parse_rpm_limit(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        rpm_limit = _parse_rpm_limit(d.pop("rpm_limit", UNSET))

        def _parse_soft_budget_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        soft_budget_usd = _parse_soft_budget_usd(d.pop("soft_budget_usd", UNSET))

        limits_input = cls(
            allowed_models=allowed_models,
            budget_duration=budget_duration,
            max_budget_usd=max_budget_usd,
            max_parallel_requests=max_parallel_requests,
            metadata=metadata,
            rpm_limit=rpm_limit,
            soft_budget_usd=soft_budget_usd,
        )

        limits_input.additional_properties = d
        return limits_input

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
