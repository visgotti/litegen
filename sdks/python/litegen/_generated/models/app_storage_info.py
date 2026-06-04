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

T = TypeVar("T", bound="AppStorageInfo")


@_attrs_define
class AppStorageInfo:
    """Public view of per-app BYO storage config — NEVER includes the secret.

    Attributes:
        configured (bool):
        access_key_id_hint (Union[None, Unset, str]):
        backend (Union[None, Unset, str]):
        bucket_name (Union[None, Unset, str]):
        custom_public_url (Union[None, Unset, str]):
        endpoint_url (Union[None, Unset, str]):
        path_prefix (Union[None, Unset, str]):
        region (Union[None, Unset, str]):
        updated_at (Union[None, Unset, datetime.datetime]):
    """

    configured: bool
    access_key_id_hint: Union[None, Unset, str] = UNSET
    backend: Union[None, Unset, str] = UNSET
    bucket_name: Union[None, Unset, str] = UNSET
    custom_public_url: Union[None, Unset, str] = UNSET
    endpoint_url: Union[None, Unset, str] = UNSET
    path_prefix: Union[None, Unset, str] = UNSET
    region: Union[None, Unset, str] = UNSET
    updated_at: Union[None, Unset, datetime.datetime] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        configured = self.configured

        access_key_id_hint: Union[None, Unset, str]
        if isinstance(self.access_key_id_hint, Unset):
            access_key_id_hint = UNSET
        else:
            access_key_id_hint = self.access_key_id_hint

        backend: Union[None, Unset, str]
        if isinstance(self.backend, Unset):
            backend = UNSET
        else:
            backend = self.backend

        bucket_name: Union[None, Unset, str]
        if isinstance(self.bucket_name, Unset):
            bucket_name = UNSET
        else:
            bucket_name = self.bucket_name

        custom_public_url: Union[None, Unset, str]
        if isinstance(self.custom_public_url, Unset):
            custom_public_url = UNSET
        else:
            custom_public_url = self.custom_public_url

        endpoint_url: Union[None, Unset, str]
        if isinstance(self.endpoint_url, Unset):
            endpoint_url = UNSET
        else:
            endpoint_url = self.endpoint_url

        path_prefix: Union[None, Unset, str]
        if isinstance(self.path_prefix, Unset):
            path_prefix = UNSET
        else:
            path_prefix = self.path_prefix

        region: Union[None, Unset, str]
        if isinstance(self.region, Unset):
            region = UNSET
        else:
            region = self.region

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
                "configured": configured,
            }
        )
        if access_key_id_hint is not UNSET:
            field_dict["access_key_id_hint"] = access_key_id_hint
        if backend is not UNSET:
            field_dict["backend"] = backend
        if bucket_name is not UNSET:
            field_dict["bucket_name"] = bucket_name
        if custom_public_url is not UNSET:
            field_dict["custom_public_url"] = custom_public_url
        if endpoint_url is not UNSET:
            field_dict["endpoint_url"] = endpoint_url
        if path_prefix is not UNSET:
            field_dict["path_prefix"] = path_prefix
        if region is not UNSET:
            field_dict["region"] = region
        if updated_at is not UNSET:
            field_dict["updated_at"] = updated_at

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        configured = d.pop("configured")

        def _parse_access_key_id_hint(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        access_key_id_hint = _parse_access_key_id_hint(
            d.pop("access_key_id_hint", UNSET)
        )

        def _parse_backend(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        backend = _parse_backend(d.pop("backend", UNSET))

        def _parse_bucket_name(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        bucket_name = _parse_bucket_name(d.pop("bucket_name", UNSET))

        def _parse_custom_public_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        custom_public_url = _parse_custom_public_url(d.pop("custom_public_url", UNSET))

        def _parse_endpoint_url(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        endpoint_url = _parse_endpoint_url(d.pop("endpoint_url", UNSET))

        def _parse_path_prefix(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        path_prefix = _parse_path_prefix(d.pop("path_prefix", UNSET))

        def _parse_region(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        region = _parse_region(d.pop("region", UNSET))

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

        app_storage_info = cls(
            configured=configured,
            access_key_id_hint=access_key_id_hint,
            backend=backend,
            bucket_name=bucket_name,
            custom_public_url=custom_public_url,
            endpoint_url=endpoint_url,
            path_prefix=path_prefix,
            region=region,
            updated_at=updated_at,
        )

        app_storage_info.additional_properties = d
        return app_storage_info

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
