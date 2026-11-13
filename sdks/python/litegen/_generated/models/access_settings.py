from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.access_mode import AccessMode
from ..models.owner_scope import OwnerScope
from ..models.signed_links import SignedLinks

T = TypeVar("T", bound="AccessSettings")


@_attrs_define
class AccessSettings:
    """The stored-output settings in force.

    Attributes:
        keep_outputs (bool): Whether litegen keeps a copy of each output. `false`: answers carry
            the vendor's own link (video, 3D) or the image itself, and nothing is
            written.
        mode (AccessMode): How the URLs litegen hands out for stored files work.
        owner_scope (OwnerScope): Which API keys pass the owner check for a stored file.
        retention_days (int): Days after which kept files are deleted; 0 keeps them until deleted.
        signed_links (SignedLinks): Who signs a signed link.
        signed_url_ttl_seconds (int): How long a signed link works, in seconds.
    """

    keep_outputs: bool
    mode: AccessMode
    owner_scope: OwnerScope
    retention_days: int
    signed_links: SignedLinks
    signed_url_ttl_seconds: int
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        keep_outputs = self.keep_outputs

        mode = self.mode.value

        owner_scope = self.owner_scope.value

        retention_days = self.retention_days

        signed_links = self.signed_links.value

        signed_url_ttl_seconds = self.signed_url_ttl_seconds

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "keep_outputs": keep_outputs,
                "mode": mode,
                "owner_scope": owner_scope,
                "retention_days": retention_days,
                "signed_links": signed_links,
                "signed_url_ttl_seconds": signed_url_ttl_seconds,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        keep_outputs = d.pop("keep_outputs")

        mode = AccessMode(d.pop("mode"))

        owner_scope = OwnerScope(d.pop("owner_scope"))

        retention_days = d.pop("retention_days")

        signed_links = SignedLinks(d.pop("signed_links"))

        signed_url_ttl_seconds = d.pop("signed_url_ttl_seconds")

        access_settings = cls(
            keep_outputs=keep_outputs,
            mode=mode,
            owner_scope=owner_scope,
            retention_days=retention_days,
            signed_links=signed_links,
            signed_url_ttl_seconds=signed_url_ttl_seconds,
        )

        access_settings.additional_properties = d
        return access_settings

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
