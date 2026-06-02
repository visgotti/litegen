from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="ImageResult")


@_attrs_define
class ImageResult:
    """A single generated image in the response.

    Attributes:
        index (int): Index in the batch.
        b64_json (Union[None, Unset, str]): Base64-encoded image data (if response_format=b64_json).
        content_type (Union[Unset, str]): Content type of the image (e.g. "image/png").
        revised_prompt (Union[None, Unset, str]): Revised prompt (if provider modified the prompt, e.g. DALL-E 3).
        url (Union[None, Unset, str]): URL of the generated image (if response_format=url).
    """

    index: int
    b64_json: Union[None, Unset, str] = UNSET
    content_type: Union[Unset, str] = UNSET
    revised_prompt: Union[None, Unset, str] = UNSET
    url: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        index = self.index

        b64_json: Union[None, Unset, str]
        if isinstance(self.b64_json, Unset):
            b64_json = UNSET
        else:
            b64_json = self.b64_json

        content_type = self.content_type

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
        if revised_prompt is not UNSET:
            field_dict["revised_prompt"] = revised_prompt
        if url is not UNSET:
            field_dict["url"] = url

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

        image_result = cls(
            index=index,
            b64_json=b64_json,
            content_type=content_type,
            revised_prompt=revised_prompt,
            url=url,
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
