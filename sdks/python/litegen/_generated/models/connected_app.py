import datetime
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
from dateutil.parser import isoparse

from ..types import UNSET, Unset

T = TypeVar("T", bound="ConnectedApp")


@_attrs_define
class ConnectedApp:
    """An app a person signed in to LiteGen with, and what it may do.

    Attributes:
        client_id (str):
        client_name (str):
        created_at (datetime.datetime):
        id (str): The grant's id: revoke the app with it.
        org_id (str):
        scopes (List[str]):
        surface (str): The endpoint it may use: `ai` (`/mcp`) or `admin` (`/admin/mcp`).
        app_id (Union[None, Unset, str]):
        last_used_at (Union[None, Unset, datetime.datetime]): When it last called LiteGen (recorded at most once a
            minute).
    """

    client_id: str
    client_name: str
    created_at: datetime.datetime
    id: str
    org_id: str
    scopes: List[str]
    surface: str
    app_id: Union[None, Unset, str] = UNSET
    last_used_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        client_id = self.client_id

        client_name = self.client_name

        created_at = self.created_at.isoformat()

        id = self.id

        org_id = self.org_id

        scopes = self.scopes

        surface = self.surface

        app_id: Union[None, Unset, str]
        if isinstance(self.app_id, Unset):
            app_id = UNSET
        else:
            app_id = self.app_id

        last_used_at: Union[None, Unset, str]
        if isinstance(self.last_used_at, Unset):
            last_used_at = UNSET
        elif isinstance(self.last_used_at, datetime.datetime):
            last_used_at = self.last_used_at.isoformat()
        else:
            last_used_at = self.last_used_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "client_id": client_id,
                "client_name": client_name,
                "created_at": created_at,
                "id": id,
                "org_id": org_id,
                "scopes": scopes,
                "surface": surface,
            }
        )
        if app_id is not UNSET:
            field_dict["app_id"] = app_id
        if last_used_at is not UNSET:
            field_dict["last_used_at"] = last_used_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        client_id = d.pop("client_id")

        client_name = d.pop("client_name")

        created_at = isoparse(d.pop("created_at"))

        id = d.pop("id")

        org_id = d.pop("org_id")

        scopes = cast(List[str], d.pop("scopes"))

        surface = d.pop("surface")

        def _parse_app_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        app_id = _parse_app_id(d.pop("app_id", UNSET))

        def _parse_last_used_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                last_used_at_type_0 = isoparse(data)

                return last_used_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        last_used_at = _parse_last_used_at(d.pop("last_used_at", UNSET))

        connected_app = cls(
            client_id=client_id,
            client_name=client_name,
            created_at=created_at,
            id=id,
            org_id=org_id,
            scopes=scopes,
            surface=surface,
            app_id=app_id,
            last_used_at=last_used_at,
        )

        connected_app.additional_properties = d
        return connected_app

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
