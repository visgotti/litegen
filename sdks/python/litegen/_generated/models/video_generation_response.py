from typing import (
    TYPE_CHECKING,
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

from ..models.generation_status import GenerationStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.usage_info import UsageInfo


T = TypeVar("T", bound="VideoGenerationResponse")


@_attrs_define
class VideoGenerationResponse:
    """Video generation can be async — this is the initial response.

    Attributes:
        created (int): Unix timestamp.
        id (str): Unique generation ID for polling.
        model (str): The model used.
        progress (int): Progress percentage (0-100).
        provider (str): The provider.
        status (GenerationStatus):
        error (Union[None, Unset, str]): Error message if failed.
        usage (Union['UsageInfo', None, Unset]):
        video_url (Union[None, Unset, str]): Video URL when completed.
    """

    created: int
    id: str
    model: str
    progress: int
    provider: str
    status: GenerationStatus
    error: Union[None, Unset, str] = UNSET
    usage: Union["UsageInfo", None, Unset] = UNSET
    video_url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.usage_info import UsageInfo

        created = self.created

        id = self.id

        model = self.model

        progress = self.progress

        provider = self.provider

        status = self.status.value

        error: Union[None, Unset, str]
        if isinstance(self.error, Unset):
            error = UNSET
        else:
            error = self.error

        usage: Union[Dict[str, Any], None, Unset]
        if isinstance(self.usage, Unset):
            usage = UNSET
        elif isinstance(self.usage, UsageInfo):
            usage = self.usage.to_dict()
        else:
            usage = self.usage

        video_url: Union[None, Unset, str]
        if isinstance(self.video_url, Unset):
            video_url = UNSET
        else:
            video_url = self.video_url

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "id": id,
                "model": model,
                "progress": progress,
                "provider": provider,
                "status": status,
            }
        )
        if error is not UNSET:
            field_dict["error"] = error
        if usage is not UNSET:
            field_dict["usage"] = usage
        if video_url is not UNSET:
            field_dict["video_url"] = video_url

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.usage_info import UsageInfo

        d = src_dict.copy()
        created = d.pop("created")

        id = d.pop("id")

        model = d.pop("model")

        progress = d.pop("progress")

        provider = d.pop("provider")

        status = GenerationStatus(d.pop("status"))

        def _parse_error(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        error = _parse_error(d.pop("error", UNSET))

        def _parse_usage(data: object) -> Union["UsageInfo", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                usage_type_1 = UsageInfo.from_dict(data)

                return usage_type_1
            except:  # noqa: E722
                pass
            return cast(Union["UsageInfo", None, Unset], data)

        usage = _parse_usage(d.pop("usage", UNSET))

        def _parse_video_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        video_url = _parse_video_url(d.pop("video_url", UNSET))

        video_generation_response = cls(
            created=created,
            id=id,
            model=model,
            progress=progress,
            provider=provider,
            status=status,
            error=error,
            usage=usage,
            video_url=video_url,
        )

        video_generation_response.additional_properties = d
        return video_generation_response

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
