from typing import Any, Dict, Type, TypeVar, Union, cast

from attrs import define as _attrs_define

from ..models.access_mode import AccessMode
from ..models.owner_scope import OwnerScope
from ..models.signed_links import SignedLinks
from ..types import UNSET, Unset

T = TypeVar("T", bound="PutAppAccessRequest")


@_attrs_define
class PutAppAccessRequest:
    """`PUT /v1/apps/{app_id}/access`: the app's settings, every one replaced.
    An omitted or `null` field uses the gateway's value; `{}` clears them all.

        Attributes:
            keep_outputs (Union[None, Unset, bool]): `false` keeps nothing: answers carry the vendor's own link, or the
                image itself.
            mode (Union[AccessMode, None, Unset]):
            owner_scope (Union[None, OwnerScope, Unset]):
            retention_days (Union[None, Unset, int]): 0 to 36500; 0 keeps files until they are deleted.
            signed_links (Union[None, SignedLinks, Unset]):
            signed_url_ttl_seconds (Union[None, Unset, int]): 60 to 604800.
    """

    keep_outputs: Union[None, Unset, bool] = UNSET
    mode: Union[AccessMode, None, Unset] = UNSET
    owner_scope: Union[None, OwnerScope, Unset] = UNSET
    retention_days: Union[None, Unset, int] = UNSET
    signed_links: Union[None, SignedLinks, Unset] = UNSET
    signed_url_ttl_seconds: Union[None, Unset, int] = UNSET

    def to_dict(self) -> Dict[str, Any]:
        keep_outputs: Union[None, Unset, bool]
        if isinstance(self.keep_outputs, Unset):
            keep_outputs = UNSET
        else:
            keep_outputs = self.keep_outputs

        mode: Union[None, Unset, str]
        if isinstance(self.mode, Unset):
            mode = UNSET
        elif isinstance(self.mode, AccessMode):
            mode = self.mode.value
        else:
            mode = self.mode

        owner_scope: Union[None, Unset, str]
        if isinstance(self.owner_scope, Unset):
            owner_scope = UNSET
        elif isinstance(self.owner_scope, OwnerScope):
            owner_scope = self.owner_scope.value
        else:
            owner_scope = self.owner_scope

        retention_days: Union[None, Unset, int]
        if isinstance(self.retention_days, Unset):
            retention_days = UNSET
        else:
            retention_days = self.retention_days

        signed_links: Union[None, Unset, str]
        if isinstance(self.signed_links, Unset):
            signed_links = UNSET
        elif isinstance(self.signed_links, SignedLinks):
            signed_links = self.signed_links.value
        else:
            signed_links = self.signed_links

        signed_url_ttl_seconds: Union[None, Unset, int]
        if isinstance(self.signed_url_ttl_seconds, Unset):
            signed_url_ttl_seconds = UNSET
        else:
            signed_url_ttl_seconds = self.signed_url_ttl_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update({})
        if keep_outputs is not UNSET:
            field_dict["keep_outputs"] = keep_outputs
        if mode is not UNSET:
            field_dict["mode"] = mode
        if owner_scope is not UNSET:
            field_dict["owner_scope"] = owner_scope
        if retention_days is not UNSET:
            field_dict["retention_days"] = retention_days
        if signed_links is not UNSET:
            field_dict["signed_links"] = signed_links
        if signed_url_ttl_seconds is not UNSET:
            field_dict["signed_url_ttl_seconds"] = signed_url_ttl_seconds

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()

        def _parse_keep_outputs(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        keep_outputs = _parse_keep_outputs(d.pop("keep_outputs", UNSET))

        def _parse_mode(data: object) -> Union[AccessMode, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                mode_type_1 = AccessMode(data)

                return mode_type_1
            except:  # noqa: E722
                pass
            return cast(Union[AccessMode, None, Unset], data)

        mode = _parse_mode(d.pop("mode", UNSET))

        def _parse_owner_scope(data: object) -> Union[None, OwnerScope, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                owner_scope_type_1 = OwnerScope(data)

                return owner_scope_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, OwnerScope, Unset], data)

        owner_scope = _parse_owner_scope(d.pop("owner_scope", UNSET))

        def _parse_retention_days(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        retention_days = _parse_retention_days(d.pop("retention_days", UNSET))

        def _parse_signed_links(data: object) -> Union[None, SignedLinks, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                signed_links_type_1 = SignedLinks(data)

                return signed_links_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, SignedLinks, Unset], data)

        signed_links = _parse_signed_links(d.pop("signed_links", UNSET))

        def _parse_signed_url_ttl_seconds(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        signed_url_ttl_seconds = _parse_signed_url_ttl_seconds(
            d.pop("signed_url_ttl_seconds", UNSET)
        )

        put_app_access_request = cls(
            keep_outputs=keep_outputs,
            mode=mode,
            owner_scope=owner_scope,
            retention_days=retention_days,
            signed_links=signed_links,
            signed_url_ttl_seconds=signed_url_ttl_seconds,
        )

        return put_app_access_request
