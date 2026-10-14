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
    from ..models.limits_input import LimitsInput


T = TypeVar("T", bound="CreateEndUserRequest")


@_attrs_define
class CreateEndUserRequest:
    """Request body for `POST /v1/end-users`.

    Attributes:
        id (str):
        alias (Union[None, Unset, str]):
        blocked (Union[Unset, bool]):
        limits (Union['LimitsInput', None, Unset]):
        metadata (Union[Unset, Any]):
    """

    id: str
    alias: Union[None, Unset, str] = UNSET
    blocked: Union[Unset, bool] = UNSET
    limits: Union["LimitsInput", None, Unset] = UNSET
    metadata: Union[Unset, Any] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.limits_input import LimitsInput

        id = self.id

        alias: Union[None, Unset, str]
        if isinstance(self.alias, Unset):
            alias = UNSET
        else:
            alias = self.alias

        blocked = self.blocked

        limits: Union[Dict[str, Any], None, Unset]
        if isinstance(self.limits, Unset):
            limits = UNSET
        elif isinstance(self.limits, LimitsInput):
            limits = self.limits.to_dict()
        else:
            limits = self.limits

        metadata = self.metadata

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "id": id,
            }
        )
        if alias is not UNSET:
            field_dict["alias"] = alias
        if blocked is not UNSET:
            field_dict["blocked"] = blocked
        if limits is not UNSET:
            field_dict["limits"] = limits
        if metadata is not UNSET:
            field_dict["metadata"] = metadata

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.limits_input import LimitsInput

        d = src_dict.copy()
        id = d.pop("id")

        def _parse_alias(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        alias = _parse_alias(d.pop("alias", UNSET))

        blocked = d.pop("blocked", UNSET)

        def _parse_limits(data: object) -> Union["LimitsInput", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                limits_type_1 = LimitsInput.from_dict(data)

                return limits_type_1
            except:  # noqa: E722
                pass
            return cast(Union["LimitsInput", None, Unset], data)

        limits = _parse_limits(d.pop("limits", UNSET))

        metadata = d.pop("metadata", UNSET)

        create_end_user_request = cls(
            id=id,
            alias=alias,
            blocked=blocked,
            limits=limits,
            metadata=metadata,
        )

        create_end_user_request.additional_properties = d
        return create_end_user_request

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
