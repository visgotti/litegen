import datetime
from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="PaginatedResponseAuditLogEntryDataItem")


@_attrs_define
class PaginatedResponseAuditLogEntryDataItem:
    """One audit log entry recording an admin action.

    Attributes:
        action (str): Action identifier, e.g. `"key.create"`, `"generation.cancel"`.
        actor_label (str): Who acted: a signed-in person's email, an API key's id (as in
            `actor_key_id`), `master-key` for the master key, or `system` for
            litegen itself (a budget alert). Since 2026-11-15 an account's email
            must be an address, so a new account can't be labelled `master-key`
            or `system`.
        created_at (datetime.datetime):
        id (str):
        target_id (str): Target entity ID.
        target_type (str): What the action changed: `api_key`, `generation`, `user`,
            `invitation`, `membership`, `organization`, `application`,
            `provider_credential`, `end_user`, `oauth_client`, a budget
            (`key_limits`, `org_limits`, `app_limits`, `member_limits`,
            `end_user_limits`), or the gateway's `model`, `provider`, `routes` and
            `settings`.
        actor_key_id (Union[None, Unset, str]): The API key that acted. Null for the master key, a signed-in person
            (a browser session, or an app signed in with OAuth) and litegen
            itself.
        after_json (Union[None, Unset, str]): JSON snapshot of the entity state *after* the action (None on
            delete/revoke).
        before_json (Union[None, Unset, str]): JSON snapshot of the entity state *before* the action (None on create).
        org_id (Union[None, Unset, str]): The organization this entry is filed under: the one whose object the
            action changed, else the one the caller acted in. Absent for an entry
            filed under no org: in hosted mode, the master key's changes to the
            gateway and to accounts, and an account change by a person who belongs
            to no org. Only the master key reads those.
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

        paginated_response_audit_log_entry_data_item = cls(
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

        paginated_response_audit_log_entry_data_item.additional_properties = d
        return paginated_response_audit_log_entry_data_item

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
