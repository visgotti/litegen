import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.entity_limits import EntityLimits


T = TypeVar("T", bound="EndUserView")


@_attrs_define
class EndUserView:
    """End user plus its limits, as returned by the end-user endpoints.

    Attributes:
        blocked (bool): A blocked end user's requests are refused with 403 `end_user_blocked`.
        created_at (datetime.datetime):
        id (str):
        org_id (str):
        updated_at (datetime.datetime):
        alias (Union[None, Unset, str]):
        last_seen_at (Union[None, Unset, datetime.datetime]):
        metadata (Union[Unset, Any]):
        limits (Union['EntityLimits', None, Unset]):
    """

    blocked: bool
    created_at: datetime.datetime
    id: str
    org_id: str
    updated_at: datetime.datetime
    alias: Union[None, Unset, str] = UNSET
    last_seen_at: Union[None, Unset, datetime.datetime] = UNSET
    metadata: Union[Unset, Any] = UNSET
    limits: Union["EntityLimits", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.entity_limits import EntityLimits

        blocked = self.blocked

        created_at = self.created_at.isoformat()

        id = self.id

        org_id = self.org_id

        updated_at = self.updated_at.isoformat()

        alias: Union[None, Unset, str]
        if isinstance(self.alias, Unset):
            alias = UNSET
        else:
            alias = self.alias

        last_seen_at: Union[None, Unset, str]
        if isinstance(self.last_seen_at, Unset):
            last_seen_at = UNSET
        elif isinstance(self.last_seen_at, datetime.datetime):
            last_seen_at = self.last_seen_at.isoformat()
        else:
            last_seen_at = self.last_seen_at

        metadata = self.metadata

        limits: Union[Dict[str, Any], None, Unset]
        if isinstance(self.limits, Unset):
            limits = UNSET
        elif isinstance(self.limits, EntityLimits):
            limits = self.limits.to_dict()
        else:
            limits = self.limits

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "blocked": blocked,
                "created_at": created_at,
                "id": id,
                "org_id": org_id,
                "updated_at": updated_at,
            }
        )
        if alias is not UNSET:
            field_dict["alias"] = alias
        if last_seen_at is not UNSET:
            field_dict["last_seen_at"] = last_seen_at
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if limits is not UNSET:
            field_dict["limits"] = limits

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.entity_limits import EntityLimits

        d = src_dict.copy()
        blocked = d.pop("blocked")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        org_id = d.pop("org_id")

        updated_at = isoparse(d.pop("updated_at"))

        def _parse_alias(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        alias = _parse_alias(d.pop("alias", UNSET))

        def _parse_last_seen_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_seen_at_type_0 = isoparse(data)

                return last_seen_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        last_seen_at = _parse_last_seen_at(d.pop("last_seen_at", UNSET))

        metadata = d.pop("metadata", UNSET)

        def _parse_limits(data: object) -> Union["EntityLimits", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                limits_type_1 = EntityLimits.from_dict(data)

                return limits_type_1
            except:  # noqa: E722
                pass
            return cast(Union["EntityLimits", None, Unset], data)

        limits = _parse_limits(d.pop("limits", UNSET))

        end_user_view = cls(
            blocked=blocked,
            created_at=created_at,
            id=id,
            org_id=org_id,
            updated_at=updated_at,
            alias=alias,
            last_seen_at=last_seen_at,
            metadata=metadata,
            limits=limits,
        )

        end_user_view.additional_properties = d
        return end_user_view

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
