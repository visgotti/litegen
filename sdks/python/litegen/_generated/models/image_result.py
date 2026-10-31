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

from ..models.image_view import ImageView
from ..types import UNSET, Unset

T = TypeVar("T", bound="ImageResult")


@_attrs_define
class ImageResult:
    """A single generated image in the response.

    Attributes:
        index (int): Index in the batch.
        b64_json (Union[None, Unset, str]): Base64-encoded image data (if response_format=b64_json).
        content_type (Union[Unset, str]): Content type of the image (e.g. "image/png").
        direction (Union[None, Unset, str]): The facing this image shows, from a `directions` request: `south`
            (towards the viewer), `south-east`, `east`, … (PixelGenie L16).
        height (Union[None, Unset, int]):
        revised_prompt (Union[None, Unset, str]): Revised prompt (if provider modified the prompt, e.g. DALL-E 3).
        url (Union[None, Unset, str]): URL of the generated image (if response_format=url).
        view (Union[ImageView, None, Unset]):
        width (Union[None, Unset, int]): The image's size in pixels, read from its bytes.
        yaw_deg (Union[None, Unset, float]): The camera's yaw for `view`, in degrees (see `ImageView`).
    """

    index: int
    b64_json: Union[None, Unset, str] = UNSET
    content_type: Union[Unset, str] = UNSET
    direction: Union[None, Unset, str] = UNSET
    height: Union[None, Unset, int] = UNSET
    revised_prompt: Union[None, Unset, str] = UNSET
    url: Union[None, Unset, str] = UNSET
    view: Union[ImageView, None, Unset] = UNSET
    width: Union[None, Unset, int] = UNSET
    yaw_deg: Union[None, Unset, float] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        index = self.index

        b64_json: Union[None, Unset, str]
        if isinstance(self.b64_json, Unset):
            b64_json = UNSET
        else:
            b64_json = self.b64_json

        content_type = self.content_type

        direction: Union[None, Unset, str]
        if isinstance(self.direction, Unset):
            direction = UNSET
        else:
            direction = self.direction

        height: Union[None, Unset, int]
        if isinstance(self.height, Unset):
            height = UNSET
        else:
            height = self.height

        revised_prompt: Union[None, Unset, str]
        if isinstance(self.revised_prompt, Unset):
            revised_prompt = UNSET
        else:
            revised_prompt = self.revised_prompt

        url: Union[None, Unset, str]
        if isinstance(self.url, Unset):
            url = UNSET
        else:
            url = self.url

        view: Union[None, Unset, str]
        if isinstance(self.view, Unset):
            view = UNSET
        elif isinstance(self.view, ImageView):
            view = self.view.value
        else:
            view = self.view

        width: Union[None, Unset, int]
        if isinstance(self.width, Unset):
            width = UNSET
        else:
            width = self.width

        yaw_deg: Union[None, Unset, float]
        if isinstance(self.yaw_deg, Unset):
            yaw_deg = UNSET
        else:
            yaw_deg = self.yaw_deg

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "index": index,
            }
        )
        if b64_json is not UNSET:
            field_dict["b64_json"] = b64_json
        if content_type is not UNSET:
            field_dict["content_type"] = content_type
        if direction is not UNSET:
            field_dict["direction"] = direction
        if height is not UNSET:
            field_dict["height"] = height
        if revised_prompt is not UNSET:
            field_dict["revised_prompt"] = revised_prompt
        if url is not UNSET:
            field_dict["url"] = url
        if view is not UNSET:
            field_dict["view"] = view
        if width is not UNSET:
            field_dict["width"] = width
        if yaw_deg is not UNSET:
            field_dict["yaw_deg"] = yaw_deg

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        index = d.pop("index")

        def _parse_b64_json(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        b64_json = _parse_b64_json(d.pop("b64_json", UNSET))

        content_type = d.pop("content_type", UNSET)

        def _parse_direction(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        direction = _parse_direction(d.pop("direction", UNSET))

        def _parse_height(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        height = _parse_height(d.pop("height", UNSET))

        def _parse_revised_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        revised_prompt = _parse_revised_prompt(d.pop("revised_prompt", UNSET))

        def _parse_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        url = _parse_url(d.pop("url", UNSET))

        def _parse_view(data: object) -> Union[ImageView, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                view_type_1 = ImageView(data)

                return view_type_1
            except:  # noqa: E722
                pass
            return cast(Union[ImageView, None, Unset], data)

        view = _parse_view(d.pop("view", UNSET))

        def _parse_width(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        width = _parse_width(d.pop("width", UNSET))

        def _parse_yaw_deg(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        yaw_deg = _parse_yaw_deg(d.pop("yaw_deg", UNSET))

        image_result = cls(
            index=index,
            b64_json=b64_json,
            content_type=content_type,
            direction=direction,
            height=height,
            revised_prompt=revised_prompt,
            url=url,
            view=view,
            width=width,
            yaw_deg=yaw_deg,
        )

        image_result.additional_properties = d
        return image_result

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
