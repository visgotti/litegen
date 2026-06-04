from typing import Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

T = TypeVar("T", bound="PutAppStorageRequest")


@_attrs_define
class PutAppStorageRequest:
    """
    Attributes:
        bucket_name (str):
        access_key_id (Union[None, Unset, str]): Write-only. Provide WITH `secret_access_key` to set/rotate; omit BOTH
            to keep existing.
        backend (Union[None, Unset, str]):
        custom_public_url (Union[None, Unset, str]):
        endpoint_url (Union[None, Unset, str]):
        path_prefix (Union[None, Unset, str]):
        region (Union[None, Unset, str]):
        secret_access_key (Union[None, Unset, str]): Write-only.
    """

    bucket_name: str
    access_key_id: Union[None, Unset, str] = UNSET
    backend: Union[None, Unset, str] = UNSET
    custom_public_url: Union[None, Unset, str] = UNSET
    endpoint_url: Union[None, Unset, str] = UNSET
    path_prefix: Union[None, Unset, str] = UNSET
    region: Union[None, Unset, str] = UNSET
    secret_access_key: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        bucket_name = self.bucket_name

        access_key_id: Union[None, Unset, str]
        if isinstance(self.access_key_id, Unset):
            access_key_id = UNSET
        else:
            access_key_id = self.access_key_id

        backend: Union[None, Unset, str]
        if isinstance(self.backend, Unset):
            backend = UNSET
        else:
            backend = self.backend

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

        secret_access_key: Union[None, Unset, str]
        if isinstance(self.secret_access_key, Unset):
            secret_access_key = UNSET
        else:
            secret_access_key = self.secret_access_key

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bucket_name": bucket_name,
            }
        )
        if access_key_id is not UNSET:
            field_dict["access_key_id"] = access_key_id
        if backend is not UNSET:
            field_dict["backend"] = backend
        if custom_public_url is not UNSET:
            field_dict["custom_public_url"] = custom_public_url
        if endpoint_url is not UNSET:
            field_dict["endpoint_url"] = endpoint_url
        if path_prefix is not UNSET:
            field_dict["path_prefix"] = path_prefix
        if region is not UNSET:
            field_dict["region"] = region
        if secret_access_key is not UNSET:
            field_dict["secret_access_key"] = secret_access_key

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        bucket_name = d.pop("bucket_name")

        def _parse_access_key_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        access_key_id = _parse_access_key_id(d.pop("access_key_id", UNSET))

        def _parse_backend(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        backend = _parse_backend(d.pop("backend", UNSET))

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

        def _parse_secret_access_key(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        secret_access_key = _parse_secret_access_key(d.pop("secret_access_key", UNSET))

        put_app_storage_request = cls(
            bucket_name=bucket_name,
            access_key_id=access_key_id,
            backend=backend,
            custom_public_url=custom_public_url,
            endpoint_url=endpoint_url,
            path_prefix=path_prefix,
            region=region,
            secret_access_key=secret_access_key,
        )

        put_app_storage_request.additional_properties = d
        return put_app_storage_request

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
