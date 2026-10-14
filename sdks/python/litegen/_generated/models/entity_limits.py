import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.entity_type import EntityType
from ..types import UNSET, Unset

T = TypeVar("T", bound="EntityLimits")


@_attrs_define
class EntityLimits:
    """Budget and access limits for one key, org, app, org member or end user.

    `spend_usd` is the spend inside the current budget window. With a
    `budget_duration` the window resets (spend back to 0) at `budget_reset_at`;
    without one the budget is a lifetime cap.

        Attributes:
            created_at (datetime.datetime):
            entity_id (str):
            entity_type (EntityType): Which principal a limits row belongs to.
            spend_usd (float): Spend inside the current window, including in-flight reservations.
            updated_at (datetime.datetime):
            allowed_models (Union[List[str], None, Unset]): Model ids this principal may generate with. None = no
                restriction.
            budget_duration (Union[None, Unset, str]): Window length: `30s`, `30m`, `12h`, `30d`, `1w` or `1mo`. None =
                never resets.
            budget_reset_at (Union[None, Unset, datetime.datetime]): When the current window ends and `spend_usd` resets.
            max_budget_usd (Union[None, Unset, float]): Hard cap in USD for the current window. Requests that would exceed
                it
                are refused with 402 `budget_exceeded`. None = unlimited.
            max_parallel_requests (Union[None, Unset, int]): Concurrent in-flight generation cap. None = unlimited.
            metadata (Union[Unset, Any]): Free-form JSON object for the caller's own bookkeeping.
            org_id (Union[None, Unset, str]): Owning org (None only for keys minted with no tenant).
            rpm_limit (Union[None, Unset, int]): Requests-per-minute cap. None = unlimited.
            soft_budget_alerted_at (Union[None, Unset, datetime.datetime]): When the soft budget was crossed in the current
                window, if it was.
            soft_budget_usd (Union[None, Unset, float]): Alert threshold in USD. Crossing it records a
                `budget.soft_limit_reached`
                audit entry and fires the gateway's alert webhook once per window.
    """

    created_at: datetime.datetime
    entity_id: str
    entity_type: EntityType
    spend_usd: float
    updated_at: datetime.datetime
    allowed_models: Union[List[str], None, Unset] = UNSET
    budget_duration: Union[None, Unset, str] = UNSET
    budget_reset_at: Union[None, Unset, datetime.datetime] = UNSET
    max_budget_usd: Union[None, Unset, float] = UNSET
    max_parallel_requests: Union[None, Unset, int] = UNSET
    metadata: Union[Unset, Any] = UNSET
    org_id: Union[None, Unset, str] = UNSET
    rpm_limit: Union[None, Unset, int] = UNSET
    soft_budget_alerted_at: Union[None, Unset, datetime.datetime] = UNSET
    soft_budget_usd: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        entity_id = self.entity_id

        entity_type = self.entity_type.value

        spend_usd = self.spend_usd

        updated_at = self.updated_at.isoformat()

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

        budget_reset_at: Union[None, Unset, str]
        if isinstance(self.budget_reset_at, Unset):
            budget_reset_at = UNSET
        elif isinstance(self.budget_reset_at, datetime.datetime):
            budget_reset_at = self.budget_reset_at.isoformat()
        else:
            budget_reset_at = self.budget_reset_at

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

        org_id: Union[None, Unset, str]
        if isinstance(self.org_id, Unset):
            org_id = UNSET
        else:
            org_id = self.org_id

        rpm_limit: Union[None, Unset, int]
        if isinstance(self.rpm_limit, Unset):
            rpm_limit = UNSET
        else:
            rpm_limit = self.rpm_limit

        soft_budget_alerted_at: Union[None, Unset, str]
        if isinstance(self.soft_budget_alerted_at, Unset):
            soft_budget_alerted_at = UNSET
        elif isinstance(self.soft_budget_alerted_at, datetime.datetime):
            soft_budget_alerted_at = self.soft_budget_alerted_at.isoformat()
        else:
            soft_budget_alerted_at = self.soft_budget_alerted_at

        soft_budget_usd: Union[None, Unset, float]
        if isinstance(self.soft_budget_usd, Unset):
            soft_budget_usd = UNSET
        else:
            soft_budget_usd = self.soft_budget_usd

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "entity_id": entity_id,
                "entity_type": entity_type,
                "spend_usd": spend_usd,
                "updated_at": updated_at,
            }
        )
        if allowed_models is not UNSET:
            field_dict["allowed_models"] = allowed_models
        if budget_duration is not UNSET:
            field_dict["budget_duration"] = budget_duration
        if budget_reset_at is not UNSET:
            field_dict["budget_reset_at"] = budget_reset_at
        if max_budget_usd is not UNSET:
            field_dict["max_budget_usd"] = max_budget_usd
        if max_parallel_requests is not UNSET:
            field_dict["max_parallel_requests"] = max_parallel_requests
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if org_id is not UNSET:
            field_dict["org_id"] = org_id
        if rpm_limit is not UNSET:
            field_dict["rpm_limit"] = rpm_limit
        if soft_budget_alerted_at is not UNSET:
            field_dict["soft_budget_alerted_at"] = soft_budget_alerted_at
        if soft_budget_usd is not UNSET:
            field_dict["soft_budget_usd"] = soft_budget_usd

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        entity_id = d.pop("entity_id")

        entity_type = EntityType(d.pop("entity_type"))

        spend_usd = d.pop("spend_usd")

        updated_at = isoparse(d.pop("updated_at"))

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

        def _parse_budget_reset_at(
            data: object,
        ) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                budget_reset_at_type_0 = isoparse(data)

                return budget_reset_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        budget_reset_at = _parse_budget_reset_at(d.pop("budget_reset_at", UNSET))

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

        def _parse_org_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_id = _parse_org_id(d.pop("org_id", UNSET))

        def _parse_rpm_limit(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        rpm_limit = _parse_rpm_limit(d.pop("rpm_limit", UNSET))

        def _parse_soft_budget_alerted_at(
            data: object,
        ) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                soft_budget_alerted_at_type_0 = isoparse(data)

                return soft_budget_alerted_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        soft_budget_alerted_at = _parse_soft_budget_alerted_at(
            d.pop("soft_budget_alerted_at", UNSET)
        )

        def _parse_soft_budget_usd(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        soft_budget_usd = _parse_soft_budget_usd(d.pop("soft_budget_usd", UNSET))

        entity_limits = cls(
            created_at=created_at,
            entity_id=entity_id,
            entity_type=entity_type,
            spend_usd=spend_usd,
            updated_at=updated_at,
            allowed_models=allowed_models,
            budget_duration=budget_duration,
            budget_reset_at=budget_reset_at,
            max_budget_usd=max_budget_usd,
            max_parallel_requests=max_parallel_requests,
            metadata=metadata,
            org_id=org_id,
            rpm_limit=rpm_limit,
            soft_budget_alerted_at=soft_budget_alerted_at,
            soft_budget_usd=soft_budget_usd,
        )

        entity_limits.additional_properties = d
        return entity_limits

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
