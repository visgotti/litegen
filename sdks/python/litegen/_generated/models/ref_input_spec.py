from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define

from ..models.image_view import ImageView
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
        data_uri (Union[Unset, bool]): Whether the vendor takes a `data:` URI where it takes a URL (default
            yes). When it does not, litegen publishes an uploaded reference at an
            unguessable URL of its own for the vendor to fetch (L4).
        default_role (Union[None, Unset, str]):
        view_order (Union[Unset, List[ImageView]]): For a multiview-to-3D vendor that takes its views as ONE ordered
            list
            (3D doc §3E): the views in that list's order. A view needs every view
            before it, since the vendor reads them by position. Empty when the
            vendor takes each view by name and any may be omitted.
    """

    max_total: int
    provider_format: Any
    roles: "RefInputSpecRoles"
    data_uri: Union[Unset, bool] = UNSET
    default_role: Union[None, Unset, str] = UNSET
    view_order: Union[Unset, List[ImageView]] = UNSET

    def to_dict(self) -> Dict[str, Any]:
        max_total = self.max_total

        provider_format = self.provider_format

        roles = self.roles.to_dict()

        data_uri = self.data_uri

        default_role: Union[None, Unset, str]
        if isinstance(self.default_role, Unset):
            default_role = UNSET
        else:
            default_role = self.default_role

        view_order: Union[Unset, List[str]] = UNSET
        if not isinstance(self.view_order, Unset):
            view_order = []
            for view_order_item_data in self.view_order:
                view_order_item = view_order_item_data.value
                view_order.append(view_order_item)

        field_dict: Dict[str, Any] = {}
        field_dict.update(
            {
                "max_total": max_total,
                "provider_format": provider_format,
                "roles": roles,
            }
        )
        if data_uri is not UNSET:
            field_dict["data_uri"] = data_uri
        if default_role is not UNSET:
            field_dict["default_role"] = default_role
        if view_order is not UNSET:
            field_dict["view_order"] = view_order

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.ref_input_spec_roles import RefInputSpecRoles

        d = src_dict.copy()
        max_total = d.pop("max_total")

        provider_format = d.pop("provider_format")

        roles = RefInputSpecRoles.from_dict(d.pop("roles"))

        data_uri = d.pop("data_uri", UNSET)

        def _parse_default_role(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        default_role = _parse_default_role(d.pop("default_role", UNSET))

        view_order = []
        _view_order = d.pop("view_order", UNSET)
        for view_order_item_data in _view_order or []:
            view_order_item = ImageView(view_order_item_data)

            view_order.append(view_order_item)

        ref_input_spec = cls(
            max_total=max_total,
            provider_format=provider_format,
            roles=roles,
            data_uri=data_uri,
            default_role=default_role,
            view_order=view_order,
        )

        return ref_input_spec
