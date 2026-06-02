import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.generation_status import GenerationStatus
from ..models.media_type import MediaType
from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseRequestLogDataItem")


@_attrs_define
class PaginatedResponseRequestLogDataItem:
    """
    Attributes:
        cost_usd (float):
        created_at (datetime.datetime):
        id (str):
        latency_ms (int):
        media_type (MediaType):
        model (str):
        provider (str):
        status (GenerationStatus):
        error (Union[None, Unset, str]):
        metadata (Union[Unset, Any]):
    """

    cost_usd: float
    created_at: datetime.datetime
    id: str
    latency_ms: int
    media_type: MediaType
    model: str
    provider: str
    status: GenerationStatus
    error: Union[None, Unset, str] = UNSET
    metadata: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cost_usd = self.cost_usd

        created_at = self.created_at.isoformat()

        id = self.id

        latency_ms = self.latency_ms

        media_type = self.media_type.value

        model = self.model

        provider = self.provider

        status = self.status.value

        error: Union[None, Unset, str]
        if isinstance(self.error, Unset):
            error = UNSET
        else:
            error = self.error

        metadata = self.metadata

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cost_usd": cost_usd,
                "created_at": created_at,
                "id": id,
                "latency_ms": latency_ms,
                "media_type": media_type,
                "model": model,
                "provider": provider,
                "status": status,
            }
        )
        if error is not UNSET:
            field_dict["error"] = error
        if metadata is not UNSET:
            field_dict["metadata"] = metadata

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        cost_usd = d.pop("cost_usd")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        latency_ms = d.pop("latency_ms")

        media_type = MediaType(d.pop("media_type"))

        model = d.pop("model")

        provider = d.pop("provider")

        status = GenerationStatus(d.pop("status"))

        def _parse_error(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        error = _parse_error(d.pop("error", UNSET))

        metadata = d.pop("metadata", UNSET)

        paginated_response_request_log_data_item = cls(
            cost_usd=cost_usd,
            created_at=created_at,
            id=id,
            latency_ms=latency_ms,
            media_type=media_type,
            model=model,
            provider=provider,
            status=status,
            error=error,
            metadata=metadata,
        )

        paginated_response_request_log_data_item.additional_properties = d
        return paginated_response_request_log_data_item

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
