import datetime
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
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="ProviderHealth")


@_attrs_define
class ProviderHealth:
    """
    Attributes:
        healthy (bool):
        provider (str):
        last_checked (Union[None, Unset, datetime.datetime]):
        latency_ms (Union[None, Unset, int]):
        message (Union[None, Unset, str]):
    """

    healthy: bool
    provider: str
    last_checked: Union[None, Unset, datetime.datetime] = UNSET
    latency_ms: Union[None, Unset, int] = UNSET
    message: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        healthy = self.healthy

        provider = self.provider

        last_checked: Union[None, Unset, str]
        if isinstance(self.last_checked, Unset):
            last_checked = UNSET
        elif isinstance(self.last_checked, datetime.datetime):
            last_checked = self.last_checked.isoformat()
        else:
            last_checked = self.last_checked

        latency_ms: Union[None, Unset, int]
        if isinstance(self.latency_ms, Unset):
            latency_ms = UNSET
        else:
            latency_ms = self.latency_ms

        message: Union[None, Unset, str]
        if isinstance(self.message, Unset):
            message = UNSET
        else:
            message = self.message

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "healthy": healthy,
                "provider": provider,
            }
        )
        if last_checked is not UNSET:
            field_dict["last_checked"] = last_checked
        if latency_ms is not UNSET:
            field_dict["latency_ms"] = latency_ms
        if message is not UNSET:
            field_dict["message"] = message

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        healthy = d.pop("healthy")

        provider = d.pop("provider")

        def _parse_last_checked(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_checked_type_0 = isoparse(data)

                return last_checked_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        last_checked = _parse_last_checked(d.pop("last_checked", UNSET))

        def _parse_latency_ms(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        latency_ms = _parse_latency_ms(d.pop("latency_ms", UNSET))

        def _parse_message(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        message = _parse_message(d.pop("message", UNSET))

        provider_health = cls(
            healthy=healthy,
            provider=provider,
            last_checked=last_checked,
            latency_ms=latency_ms,
            message=message,
        )

        provider_health.additional_properties = d
        return provider_health

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
