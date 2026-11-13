import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.generation_status import GenerationStatus
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.usage_info import UsageInfo
    from ..models.video_generation_response_metadata_type_0 import (
        VideoGenerationResponseMetadataType0,
    )
    from ..models.video_media import VideoMedia
    from ..models.video_segment import VideoSegment


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
        media (Union['VideoMedia', None, Unset]):
        metadata (Union['VideoGenerationResponseMetadataType0', None, Unset]): The request's `metadata`, as sent.
        segments (Union[List['VideoSegment'], None, Unset]): An operation's clip, stretch by stretch: from the source or
            generated
            (3D doc §5). Absent on a plain generation.
        usage (Union['UsageInfo', None, Unset]):
        video_url (Union[None, Unset, str]): Video URL when completed.
        video_url_expires_at (Union[None, Unset, datetime.datetime]): When `video_url` stops working, if it is a signed
            link
            (`docs/storage-access.md`); poll again for a fresh one.
    """

    created: int
    id: str
    model: str
    progress: int
    provider: str
    status: GenerationStatus
    error: Union[None, Unset, str] = UNSET
    media: Union["VideoMedia", None, Unset] = UNSET
    metadata: Union["VideoGenerationResponseMetadataType0", None, Unset] = UNSET
    segments: Union[List["VideoSegment"], None, Unset] = UNSET
    usage: Union["UsageInfo", None, Unset] = UNSET
    video_url: Union[None, Unset, str] = UNSET
    video_url_expires_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.usage_info import UsageInfo
        from ..models.video_generation_response_metadata_type_0 import (
            VideoGenerationResponseMetadataType0,
        )
        from ..models.video_media import VideoMedia

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

        media: Union[Dict[str, Any], None, Unset]
        if isinstance(self.media, Unset):
            media = UNSET
        elif isinstance(self.media, VideoMedia):
            media = self.media.to_dict()
        else:
            media = self.media

        metadata: Union[Dict[str, Any], None, Unset]
        if isinstance(self.metadata, Unset):
            metadata = UNSET
        elif isinstance(self.metadata, VideoGenerationResponseMetadataType0):
            metadata = self.metadata.to_dict()
        else:
            metadata = self.metadata

        segments: Union[List[Dict[str, Any]], None, Unset]
        if isinstance(self.segments, Unset):
            segments = UNSET
        elif isinstance(self.segments, list):
            segments = []
            for segments_type_0_item_data in self.segments:
                segments_type_0_item = segments_type_0_item_data.to_dict()
                segments.append(segments_type_0_item)

        else:
            segments = self.segments

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

        video_url_expires_at: Union[None, Unset, str]
        if isinstance(self.video_url_expires_at, Unset):
            video_url_expires_at = UNSET
        elif isinstance(self.video_url_expires_at, datetime.datetime):
            video_url_expires_at = self.video_url_expires_at.isoformat()
        else:
            video_url_expires_at = self.video_url_expires_at

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
        if media is not UNSET:
            field_dict["media"] = media
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if segments is not UNSET:
            field_dict["segments"] = segments
        if usage is not UNSET:
            field_dict["usage"] = usage
        if video_url is not UNSET:
            field_dict["video_url"] = video_url
        if video_url_expires_at is not UNSET:
            field_dict["video_url_expires_at"] = video_url_expires_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.usage_info import UsageInfo
        from ..models.video_generation_response_metadata_type_0 import (
            VideoGenerationResponseMetadataType0,
        )
        from ..models.video_media import VideoMedia
        from ..models.video_segment import VideoSegment

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

        def _parse_media(data: object) -> Union["VideoMedia", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                media_type_1 = VideoMedia.from_dict(data)

                return media_type_1
            except:  # noqa: E722
                pass
            return cast(Union["VideoMedia", None, Unset], data)

        media = _parse_media(d.pop("media", UNSET))

        def _parse_metadata(
            data: object,
        ) -> Union["VideoGenerationResponseMetadataType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                metadata_type_0 = VideoGenerationResponseMetadataType0.from_dict(data)

                return metadata_type_0
            except:  # noqa: E722
                pass
            return cast(
                Union["VideoGenerationResponseMetadataType0", None, Unset], data
            )

        metadata = _parse_metadata(d.pop("metadata", UNSET))

        def _parse_segments(data: object) -> Union[List["VideoSegment"], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                segments_type_0 = []
                _segments_type_0 = data
                for segments_type_0_item_data in _segments_type_0:
                    segments_type_0_item = VideoSegment.from_dict(
                        segments_type_0_item_data
                    )

                    segments_type_0.append(segments_type_0_item)

                return segments_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List["VideoSegment"], None, Unset], data)

        segments = _parse_segments(d.pop("segments", UNSET))

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

        def _parse_video_url_expires_at(
            data: object,
        ) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                video_url_expires_at_type_0 = isoparse(data)

                return video_url_expires_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        video_url_expires_at = _parse_video_url_expires_at(
            d.pop("video_url_expires_at", UNSET)
        )

        video_generation_response = cls(
            created=created,
            id=id,
            model=model,
            progress=progress,
            provider=provider,
            status=status,
            error=error,
            media=media,
            metadata=metadata,
            segments=segments,
            usage=usage,
            video_url=video_url,
            video_url_expires_at=video_url_expires_at,
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
