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

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.image_result import ImageResult
    from ..models.usage_info import UsageInfo


T = TypeVar("T", bound="ImageGenerationResponse")


@_attrs_define
class ImageGenerationResponse:
    """Response for an image generation request.
    Follows OpenAI's images response format for compatibility.

        Attributes:
            created (int): Unix timestamp of when the request was created.
            data (List['ImageResult']): Array of generated images.
            id (str): Unique request ID for tracking.
            model (str): The model that was used.
            provider (str): The provider that handled the request.
            usage (Union['UsageInfo', None, Unset]):
    """

    created: int
    data: List["ImageResult"]
    id: str
    model: str
    provider: str
    usage: Union["UsageInfo", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.usage_info import UsageInfo

        created = self.created

        data = []
        for data_item_data in self.data:
            data_item = data_item_data.to_dict()
            data.append(data_item)

        id = self.id

        model = self.model

        provider = self.provider

        usage: Union[Dict[str, Any], None, Unset]
        if isinstance(self.usage, Unset):
            usage = UNSET
        elif isinstance(self.usage, UsageInfo):
            usage = self.usage.to_dict()
        else:
            usage = self.usage

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created": created,
                "data": data,
                "id": id,
                "model": model,
                "provider": provider,
            }
        )
        if usage is not UNSET:
            field_dict["usage"] = usage

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.image_result import ImageResult
        from ..models.usage_info import UsageInfo

        d = src_dict.copy()
        created = d.pop("created")

        data = []
        _data = d.pop("data")
        for data_item_data in _data:
            data_item = ImageResult.from_dict(data_item_data)

            data.append(data_item)

        id = d.pop("id")

        model = d.pop("model")

        provider = d.pop("provider")

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

        image_generation_response = cls(
            created=created,
            data=data,
            id=id,
            model=model,
            provider=provider,
            usage=usage,
        )

        image_generation_response.additional_properties = d
        return image_generation_response

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
