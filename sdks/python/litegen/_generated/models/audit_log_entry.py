import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="AuditLogEntry")


@_attrs_define
class AuditLogEntry:
    """One audit log entry recording an admin action.

    Attributes:
        action (str): Action identifier, e.g. `"key.create"`, `"generation.cancel"`.
        actor_label (str): Human-readable actor label: `"master-key"` or the key's name.
        created_at (datetime.datetime):
        id (str):
        target_id (str): Target entity ID.
        target_type (str): Target entity type: `"api_key"` or `"generation"`.
        actor_key_id (Union[None, Unset, str]): The API key ID of the actor. None when the master key was used.
        after_json (Union[None, Unset, str]): JSON snapshot of the entity state *after* the action (None on
            delete/revoke).
        before_json (Union[None, Unset, str]): JSON snapshot of the entity state *before* the action (None on create).
        org_id (Union[None, Unset, str]): Tenant (organization) this audit entry belongs to. None falls back to the
            default org at the DB layer (single-tenant); set from the caller's active org.
    """

    action: str
    actor_label: str
    created_at: datetime.datetime
    id: str
    target_id: str
    target_type: str
    actor_key_id: Union[None, Unset, str] = UNSET
    after_json: Union[None, Unset, str] = UNSET
    before_json: Union[None, Unset, str] = UNSET
    org_id: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        action = self.action

        actor_label = self.actor_label

        created_at = self.created_at.isoformat()

        id = self.id

        target_id = self.target_id

        target_type = self.target_type

        actor_key_id: Union[None, Unset, str]
        if isinstance(self.actor_key_id, Unset):
            actor_key_id = UNSET
        else:
            actor_key_id = self.actor_key_id

        after_json: Union[None, Unset, str]
        if isinstance(self.after_json, Unset):
            after_json = UNSET
        else:
            after_json = self.after_json

        before_json: Union[None, Unset, str]
        if isinstance(self.before_json, Unset):
            before_json = UNSET
        else:
            before_json = self.before_json

        org_id: Union[None, Unset, str]
        if isinstance(self.org_id, Unset):
            org_id = UNSET
        else:
            org_id = self.org_id

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "action": action,
                "actor_label": actor_label,
                "created_at": created_at,
                "id": id,
                "target_id": target_id,
                "target_type": target_type,
            }
        )
        if actor_key_id is not UNSET:
            field_dict["actor_key_id"] = actor_key_id
        if after_json is not UNSET:
            field_dict["after_json"] = after_json
        if before_json is not UNSET:
            field_dict["before_json"] = before_json
        if org_id is not UNSET:
            field_dict["org_id"] = org_id

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        action = d.pop("action")

        actor_label = d.pop("actor_label")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        target_id = d.pop("target_id")

        target_type = d.pop("target_type")

        def _parse_actor_key_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        actor_key_id = _parse_actor_key_id(d.pop("actor_key_id", UNSET))

        def _parse_after_json(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        after_json = _parse_after_json(d.pop("after_json", UNSET))

        def _parse_before_json(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        before_json = _parse_before_json(d.pop("before_json", UNSET))

        def _parse_org_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        org_id = _parse_org_id(d.pop("org_id", UNSET))

        audit_log_entry = cls(
            action=action,
            actor_label=actor_label,
            created_at=created_at,
            id=id,
            target_id=target_id,
            target_type=target_type,
            actor_key_id=actor_key_id,
            after_json=after_json,
            before_json=before_json,
            org_id=org_id,
        )

        audit_log_entry.additional_properties = d
        return audit_log_entry

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
