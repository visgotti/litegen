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

from ..models.media_type import MediaType
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.model_capabilities import ModelCapabilities
    from ..models.model_pricing import ModelPricing


T = TypeVar("T", bound="ModelInfo")


@_attrs_define
class ModelInfo:
    """A model available through LiteGen.

    Attributes:
        capabilities (ModelCapabilities):
        id (str): Unique model ID (e.g. "openai/dall-e-3").
        is_available (bool): Whether the model is currently available.
        media_type (MediaType):
        name (str): Human-readable name.
        provider (str): Provider name.
        description (Union[Unset, str]): Description.
        pricing (Union['ModelPricing', None, Unset]):
        tags (Union[Unset, List[str]]): Tags for filtering.
    """

    capabilities: "ModelCapabilities"
    id: str
    is_available: bool
    media_type: MediaType
    name: str
    provider: str
    description: Union[Unset, str] = UNSET
    pricing: Union["ModelPricing", None, Unset] = UNSET
    tags: Union[Unset, List[str]] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.model_pricing import ModelPricing

        capabilities = self.capabilities.to_dict()

        id = self.id

        is_available = self.is_available

        media_type = self.media_type.value

        name = self.name

        provider = self.provider

        description = self.description

        pricing: Union[Dict[str, Any], None, Unset]
        if isinstance(self.pricing, Unset):
            pricing = UNSET
        elif isinstance(self.pricing, ModelPricing):
            pricing = self.pricing.to_dict()
        else:
            pricing = self.pricing

        tags: Union[Unset, List[str]] = UNSET
        if not isinstance(self.tags, Unset):
            tags = self.tags

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "capabilities": capabilities,
                "id": id,
                "is_available": is_available,
                "media_type": media_type,
                "name": name,
                "provider": provider,
            }
        )
        if description is not UNSET:
            field_dict["description"] = description
        if pricing is not UNSET:
            field_dict["pricing"] = pricing
        if tags is not UNSET:
            field_dict["tags"] = tags

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.model_capabilities import ModelCapabilities
        from ..models.model_pricing import ModelPricing

        d = src_dict.copy()
        capabilities = ModelCapabilities.from_dict(d.pop("capabilities"))

        id = d.pop("id")

        is_available = d.pop("is_available")

        media_type = MediaType(d.pop("media_type"))

        name = d.pop("name")

        provider = d.pop("provider")

        description = d.pop("description", UNSET)

        def _parse_pricing(data: object) -> Union["ModelPricing", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                pricing_type_1 = ModelPricing.from_dict(data)

                return pricing_type_1
            except:  # noqa: E722
                pass
            return cast(Union["ModelPricing", None, Unset], data)

        pricing = _parse_pricing(d.pop("pricing", UNSET))

        tags = cast(List[str], d.pop("tags", UNSET))

        model_info = cls(
            capabilities=capabilities,
            id=id,
            is_available=is_available,
            media_type=media_type,
            name=name,
            provider=provider,
            description=description,
            pricing=pricing,
            tags=tags,
        )

        model_info.additional_properties = d
        return model_info

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
