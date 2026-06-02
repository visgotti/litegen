import datetime
from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

T = TypeVar("T", bound="ApiKeyCreatedResponse")


@_attrs_define
class ApiKeyCreatedResponse:
    """Response for `POST /v1/keys`.

    Attributes:
        created_at (datetime.datetime):
        key (str):
        name (str):
        prefix (str):
    """

    created_at: datetime.datetime
    key: str
    name: str
    prefix: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        created_at = self.created_at.isoformat()

        key = self.key

        name = self.name

        prefix = self.prefix

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "created_at": created_at,
                "key": key,
                "name": name,
                "prefix": prefix,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        created_at = isoparse(d.pop("created_at"))

        key = d.pop("key")

        name = d.pop("name")

        prefix = d.pop("prefix")

        api_key_created_response = cls(
            created_at=created_at,
            key=key,
            name=name,
            prefix=prefix,
        )

        api_key_created_response.additional_properties = d
        return api_key_created_response

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
