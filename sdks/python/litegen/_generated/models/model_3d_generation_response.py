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
    from ..models.model_3d_asset import Model3DAsset
    from ..models.usage_info import UsageInfo


T = TypeVar("T", bound="Model3DGenerationResponse")


@_attrs_define
class Model3DGenerationResponse:
    """3D generation is always async — this is both the submit response and the
    poll response, mirroring `VideoGenerationResponse` with `video_url` replaced
    by the richer `assets` list.

        Attributes:
            created (int):
            id (str):
            model (str):
            progress (int): Unified 0–100 progress.
            provider (str):
            status (GenerationStatus):
            assets (Union[Unset, List['Model3DAsset']]): Populated on `completed`. Omitted entirely while the job is in
                flight.
            error (Union[None, Unset, str]):
            usage (Union['UsageInfo', None, Unset]):
    """

    created: int
    id: str
    model: str
    progress: int
    provider: str
    status: GenerationStatus
    assets: Union[Unset, List["Model3DAsset"]] = UNSET
    error: Union[None, Unset, str] = UNSET
    usage: Union["UsageInfo", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.usage_info import UsageInfo

        created = self.created

        id = self.id

        model = self.model

        progress = self.progress

        provider = self.provider

        status = self.status.value

        assets: Union[Unset, List[Dict[str, Any]]] = UNSET
        if not isinstance(self.assets, Unset):
            assets = []
            for assets_item_data in self.assets:
                assets_item = assets_item_data.to_dict()
                assets.append(assets_item)

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
        if assets is not UNSET:
            field_dict["assets"] = assets
        if error is not UNSET:
            field_dict["error"] = error
        if usage is not UNSET:
            field_dict["usage"] = usage

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.model_3d_asset import Model3DAsset
        from ..models.usage_info import UsageInfo

        d = src_dict.copy()
        created = d.pop("created")

        id = d.pop("id")

        model = d.pop("model")

        progress = d.pop("progress")

        provider = d.pop("provider")

        status = GenerationStatus(d.pop("status"))

        assets = []
        _assets = d.pop("assets", UNSET)
        for assets_item_data in _assets or []:
            assets_item = Model3DAsset.from_dict(assets_item_data)

            assets.append(assets_item)

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

        model_3d_generation_response = cls(
            created=created,
            id=id,
            model=model,
            progress=progress,
            provider=provider,
            status=status,
            assets=assets,
            error=error,
            usage=usage,
        )

        model_3d_generation_response.additional_properties = d
        return model_3d_generation_response

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
