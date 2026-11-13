import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.access_settings import AccessSettings
    from ..models.app_access_overrides import AppAccessOverrides


T = TypeVar("T", bound="AppAccessSettings")


@_attrs_define
class AppAccessSettings:
    """`GET`/`PUT /v1/apps/{app_id}/access`: who may open the app's stored files.

    Attributes:
        app_id (str):
        effective (AccessSettings): The stored-output settings in force.
        overrides (AppAccessOverrides): One app's own settings; `null` uses the gateway's value.
        links_revoked_at (Union[None, Unset, datetime.datetime]): litegen's links to the app's files issued before this
            time are refused:
            the latest of the app's revocation and the gateway's.
        updated_at (Union[None, Unset, datetime.datetime]): When the app's own settings last changed; `null` when it has
            none.
    """

    app_id: str
    effective: "AccessSettings"
    overrides: "AppAccessOverrides"
    links_revoked_at: Union[None, Unset, datetime.datetime] = UNSET
    updated_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        app_id = self.app_id

        effective = self.effective.to_dict()

        overrides = self.overrides.to_dict()

        links_revoked_at: Union[None, Unset, str]
        if isinstance(self.links_revoked_at, Unset):
            links_revoked_at = UNSET
        elif isinstance(self.links_revoked_at, datetime.datetime):
            links_revoked_at = self.links_revoked_at.isoformat()
        else:
            links_revoked_at = self.links_revoked_at

        updated_at: Union[None, Unset, str]
        if isinstance(self.updated_at, Unset):
            updated_at = UNSET
        elif isinstance(self.updated_at, datetime.datetime):
            updated_at = self.updated_at.isoformat()
        else:
            updated_at = self.updated_at

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app_id": app_id,
                "effective": effective,
                "overrides": overrides,
            }
        )
        if links_revoked_at is not UNSET:
            field_dict["links_revoked_at"] = links_revoked_at
        if updated_at is not UNSET:
            field_dict["updated_at"] = updated_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.access_settings import AccessSettings
        from ..models.app_access_overrides import AppAccessOverrides

        d = src_dict.copy()
        app_id = d.pop("app_id")

        effective = AccessSettings.from_dict(d.pop("effective"))

        overrides = AppAccessOverrides.from_dict(d.pop("overrides"))

        def _parse_links_revoked_at(
            data: object,
        ) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                links_revoked_at_type_0 = isoparse(data)

                return links_revoked_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        links_revoked_at = _parse_links_revoked_at(d.pop("links_revoked_at", UNSET))

        def _parse_updated_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                updated_at_type_0 = isoparse(data)

                return updated_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        updated_at = _parse_updated_at(d.pop("updated_at", UNSET))

        app_access_settings = cls(
            app_id=app_id,
            effective=effective,
            overrides=overrides,
            links_revoked_at=links_revoked_at,
            updated_at=updated_at,
        )

        app_access_settings.additional_properties = d
        return app_access_settings

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
