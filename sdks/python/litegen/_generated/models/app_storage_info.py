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
        credential_mode (Union[None, Unset, str]): How litegen reaches the bucket: `access_key` (a key pair) or
            `assume_role` (a cross-account IAM role).
        custom_public_url (Union[None, Unset, str]):
        endpoint_url (Union[None, Unset, str]):
        external_id (Union[None, Unset, str]): The external ID the role's trust policy must require. litegen assigns
            it, one per app; it is shown before a role is saved.
        fallback_to_litegen_store (Union[None, Unset, bool]): Whether litegen's own store keeps the app's files while
            its bucket
            can't be reached (default true). With false, they wait for the bucket.
        litegen_principal_arn (Union[None, Unset, str]): litegen's own AWS principal, which the role must trust; absent
            when
            litegen has no AWS identity.
        path_prefix (Union[None, Unset, str]):
        region (Union[None, Unset, str]):
        role_arn (Union[None, Unset, str]):
        updated_at (Union[None, Unset, datetime.datetime]):
    """

    configured: bool
    access_key_id_hint: Union[None, Unset, str] = UNSET
    backend: Union[None, Unset, str] = UNSET
    bucket_name: Union[None, Unset, str] = UNSET
    credential_mode: Union[None, Unset, str] = UNSET
    custom_public_url: Union[None, Unset, str] = UNSET
    endpoint_url: Union[None, Unset, str] = UNSET
    external_id: Union[None, Unset, str] = UNSET
    fallback_to_litegen_store: Union[None, Unset, bool] = UNSET
    litegen_principal_arn: Union[None, Unset, str] = UNSET
    path_prefix: Union[None, Unset, str] = UNSET
    region: Union[None, Unset, str] = UNSET
    role_arn: Union[None, Unset, str] = UNSET
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

        credential_mode: Union[None, Unset, str]
        if isinstance(self.credential_mode, Unset):
            credential_mode = UNSET
        else:
            credential_mode = self.credential_mode

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

        external_id: Union[None, Unset, str]
        if isinstance(self.external_id, Unset):
            external_id = UNSET
        else:
            external_id = self.external_id

        fallback_to_litegen_store: Union[None, Unset, bool]
        if isinstance(self.fallback_to_litegen_store, Unset):
            fallback_to_litegen_store = UNSET
        else:
            fallback_to_litegen_store = self.fallback_to_litegen_store

        litegen_principal_arn: Union[None, Unset, str]
        if isinstance(self.litegen_principal_arn, Unset):
            litegen_principal_arn = UNSET
        else:
            litegen_principal_arn = self.litegen_principal_arn

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

        role_arn: Union[None, Unset, str]
        if isinstance(self.role_arn, Unset):
            role_arn = UNSET
        else:
            role_arn = self.role_arn

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
        if credential_mode is not UNSET:
            field_dict["credential_mode"] = credential_mode
        if custom_public_url is not UNSET:
            field_dict["custom_public_url"] = custom_public_url
        if endpoint_url is not UNSET:
            field_dict["endpoint_url"] = endpoint_url
        if external_id is not UNSET:
            field_dict["external_id"] = external_id
        if fallback_to_litegen_store is not UNSET:
            field_dict["fallback_to_litegen_store"] = fallback_to_litegen_store
        if litegen_principal_arn is not UNSET:
            field_dict["litegen_principal_arn"] = litegen_principal_arn
        if path_prefix is not UNSET:
            field_dict["path_prefix"] = path_prefix
        if region is not UNSET:
            field_dict["region"] = region
        if role_arn is not UNSET:
            field_dict["role_arn"] = role_arn
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

        def _parse_credential_mode(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        credential_mode = _parse_credential_mode(d.pop("credential_mode", UNSET))

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

        def _parse_external_id(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        external_id = _parse_external_id(d.pop("external_id", UNSET))

        def _parse_fallback_to_litegen_store(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        fallback_to_litegen_store = _parse_fallback_to_litegen_store(
            d.pop("fallback_to_litegen_store", UNSET)
        )

        def _parse_litegen_principal_arn(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        litegen_principal_arn = _parse_litegen_principal_arn(
            d.pop("litegen_principal_arn", UNSET)
        )

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

        def _parse_role_arn(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        role_arn = _parse_role_arn(d.pop("role_arn", UNSET))

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
            credential_mode=credential_mode,
            custom_public_url=custom_public_url,
            endpoint_url=endpoint_url,
            external_id=external_id,
            fallback_to_litegen_store=fallback_to_litegen_store,
            litegen_principal_arn=litegen_principal_arn,
            path_prefix=path_prefix,
            region=region,
            role_arn=role_arn,
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
