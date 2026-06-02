from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.ref_input_spec_roles import RefInputSpecRoles


T = TypeVar("T", bound="RefInputSpec")


@_attrs_define
class RefInputSpec:
    """
    Attributes:
        max_total (int):
        provider_format (Any):
        roles (RefInputSpecRoles):
        default_role (Union[None, Unset, str]):
    """

    max_total: int
    provider_format: Any
    roles: "RefInputSpecRoles"
    default_role: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        max_total = self.max_total

        provider_format = self.provider_format

        roles = self.roles.to_dict()

        default_role: Union[None, Unset, str]
        if isinstance(self.default_role, Unset):
            default_role = UNSET
        else:
            default_role = self.default_role

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "max_total": max_total,
                "provider_format": provider_format,
                "roles": roles,
            }
        )
        if default_role is not UNSET:
            field_dict["default_role"] = default_role

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.ref_input_spec_roles import RefInputSpecRoles

        d = src_dict.copy()
        max_total = d.pop("max_total")

        provider_format = d.pop("provider_format")

        roles = RefInputSpecRoles.from_dict(d.pop("roles"))

        def _parse_default_role(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        default_role = _parse_default_role(d.pop("default_role", UNSET))

        ref_input_spec = cls(
            max_total=max_total,
            provider_format=provider_format,
            roles=roles,
            default_role=default_role,
        )

        ref_input_spec.additional_properties = d
        return ref_input_spec

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
