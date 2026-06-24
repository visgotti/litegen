import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast
from uuid import UUID

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.generation_status import GenerationStatus
from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseGenerationDataItem")


@_attrs_define
class PaginatedResponseGenerationDataItem:
    """A DB-backed generation row (video or image, currently used for video).

    Attributes:
        cost_usd (float):
        created_at (datetime.datetime):
        id (str): Locally-minted ID (e.g. "litegen-vid-<uuid>").
        media_type (str):
        model (str):
        progress (int):
        provider (str):
        status (GenerationStatus):
        app_id (Union[None, Unset, str]): Owning application. None for pre-tenancy / untenanted rows.
        completed_at (Union[None, Unset, datetime.datetime]):
        error_message (Union[None, Unset, str]):
        key_id (Union[None, UUID, Unset]): API key that submitted this generation; None when master key was used.
        metadata (Union[Unset, Any]): Arbitrary JSON metadata.
        org_id (Union[None, Unset, str]): Owning tenant (organization). None for pre-tenancy / untenanted rows.
        provider_job_id (Union[None, Unset, str]): Provider-assigned job ID used for polling.
        result_url (Union[None, Unset, str]): Final result URL when completed.
    """

    cost_usd: float
    created_at: datetime.datetime
    id: str
    media_type: str
    model: str
    progress: int
    provider: str
    status: GenerationStatus
    app_id: Union[None, Unset, str] = UNSET
    completed_at: Union[None, Unset, datetime.datetime] = UNSET
    error_message: Union[None, Unset, str] = UNSET
    key_id: Union[None, UUID, Unset] = UNSET
    metadata: Union[Unset, Any] = UNSET
    org_id: Union[None, Unset, str] = UNSET
    provider_job_id: Union[None, Unset, str] = UNSET
    result_url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        cost_usd = self.cost_usd

        created_at = self.created_at.isoformat()

        id = self.id

        media_type = self.media_type

        model = self.model

        progress = self.progress

        provider = self.provider

        status = self.status.value

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        completed_at: Union[None, Unset, str]
        if isinstance(self.completed_at, Unset):
            completed_at = UNSET
        elif isinstance(self.completed_at, datetime.datetime):
            completed_at = self.completed_at.isoformat()
        else:
            completed_at = self.completed_at

        error_message: Union[None, Unset, str]
        if isinstance(self.error_message, Unset):
            error_message = UNSET
        else:
            error_message = self.error_message

        key_id: Union[None, Unset, str]
        if isinstance(self.key_id, Unset):
            key_id = UNSET
        elif isinstance(self.key_id, UUID):
            key_id = str(self.key_id)
        else:
            key_id = self.key_id

        metadata = self.metadata

        org_id: Union[None, Unset, str]
        if isinstance(self.org_id, Unset):
            org_id = UNSET
        else:
            org_id = self.org_id

        provider_job_id: Union[None, Unset, str]
        if isinstance(self.provider_job_id, Unset):
            provider_job_id = UNSET
        else:
            provider_job_id = self.provider_job_id

        result_url: Union[None, Unset, str]
        if isinstance(self.result_url, Unset):
            result_url = UNSET
        else:
            result_url = self.result_url

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "cost_usd": cost_usd,
                "created_at": created_at,
                "id": id,
                "media_type": media_type,
                "model": model,
                "progress": progress,
                "provider": provider,
                "status": status,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id
        if completed_at is not UNSET:
            field_dict["completed_at"] = completed_at
        if error_message is not UNSET:
            field_dict["error_message"] = error_message
        if key_id is not UNSET:
            field_dict["key_id"] = key_id
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if org_id is not UNSET:
            field_dict["org_id"] = org_id
        if provider_job_id is not UNSET:
            field_dict["provider_job_id"] = provider_job_id
        if result_url is not UNSET:
            field_dict["result_url"] = result_url

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        cost_usd = d.pop("cost_usd")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        media_type = d.pop("media_type")

        model = d.pop("model")

        progress = d.pop("progress")

        provider = d.pop("provider")

        status = GenerationStatus(d.pop("status"))

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        def _parse_completed_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                completed_at_type_0 = isoparse(data)

                return completed_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        completed_at = _parse_completed_at(d.pop("completed_at", UNSET))

        def _parse_error_message(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        error_message = _parse_error_message(d.pop("error_message", UNSET))

        def _parse_key_id(data: object) -> Union[None, UUID, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                key_id_type_0 = UUID(data)

                return key_id_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, UUID, Unset], data)

        key_id = _parse_key_id(d.pop("key_id", UNSET))

        metadata = d.pop("metadata", UNSET)

        def _parse_org_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_id = _parse_org_id(d.pop("org_id", UNSET))

        def _parse_provider_job_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        provider_job_id = _parse_provider_job_id(d.pop("provider_job_id", UNSET))

        def _parse_result_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        result_url = _parse_result_url(d.pop("result_url", UNSET))

        paginated_response_generation_data_item = cls(
            cost_usd=cost_usd,
            created_at=created_at,
            id=id,
            media_type=media_type,
            model=model,
            progress=progress,
            provider=provider,
            status=status,
            app_id=app_id,
            completed_at=completed_at,
            error_message=error_message,
            key_id=key_id,
            metadata=metadata,
            org_id=org_id,
            provider_job_id=provider_job_id,
            result_url=result_url,
        )

        paginated_response_generation_data_item.additional_properties = d
        return paginated_response_generation_data_item

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
