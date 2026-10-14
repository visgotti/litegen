from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="UpdateEndUserRequest")


@_attrs_define
class UpdateEndUserRequest:
    """Request body for `PATCH /v1/end-users/{id}`. Omitted fields are unchanged.

    Attributes:
        alias (Union[None, Unset, str]):
        blocked (Union[None, Unset, bool]):
        metadata (Union[Unset, Any]):
    """

    alias: Union[None, Unset, str] = UNSET
    blocked: Union[None, Unset, bool] = UNSET
    metadata: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        alias: Union[None, Unset, str]
        if isinstance(self.alias, Unset):
            alias = UNSET
        else:
            alias = self.alias

        blocked: Union[None, Unset, bool]
        if isinstance(self.blocked, Unset):
            blocked = UNSET
        else:
            blocked = self.blocked

        metadata = self.metadata

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if alias is not UNSET:
            field_dict["alias"] = alias
        if blocked is not UNSET:
            field_dict["blocked"] = blocked
        if metadata is not UNSET:
            field_dict["metadata"] = metadata

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_alias(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        alias = _parse_alias(d.pop("alias", UNSET))

        def _parse_blocked(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        blocked = _parse_blocked(d.pop("blocked", UNSET))

        metadata = d.pop("metadata", UNSET)

        update_end_user_request = cls(
            alias=alias,
            blocked=blocked,
            metadata=metadata,
        )

        update_end_user_request.additional_properties = d
        return update_end_user_request

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
