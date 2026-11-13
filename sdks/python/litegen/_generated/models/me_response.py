from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

if TYPE_CHECKING:
    from ..models.me_org import MeOrg
    from ..models.me_user import MeUser


T = TypeVar("T", bound="MeResponse")


@_attrs_define
class MeResponse:
    """`GET /v1/auth/me`: the signed-in person, the organizations they belong to
    with their role and permissions in each, and where this request acts.

        Attributes:
            active_app (Union[None, str]): The app this request acts in (`X-Litegen-App-Id`, else the
                organization's first; an OAuth sign-in's own). Null when there is none.
            active_org (Union[None, str]): The organization this request acts in (`X-Litegen-Org-Id`, else the
                person's first; an OAuth sign-in's own). Null when there is none.
            orgs (List['MeOrg']): The organizations the person belongs to. A sign-in through OAuth lists
                only the one its grant is for.
            user (MeUser): The signed-in account.
    """

    active_app: Union[None, str]
    active_org: Union[None, str]
    orgs: List["MeOrg"]
    user: "MeUser"
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        active_app: Union[None, str]
        active_app = self.active_app

        active_org: Union[None, str]
        active_org = self.active_org

        orgs = []
        for orgs_item_data in self.orgs:
            orgs_item = orgs_item_data.to_dict()
            orgs.append(orgs_item)

        user = self.user.to_dict()

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "active_app": active_app,
                "active_org": active_org,
                "orgs": orgs,
                "user": user,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.me_org import MeOrg
        from ..models.me_user import MeUser

        d = src_dict.copy()

        def _parse_active_app(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        active_app = _parse_active_app(d.pop("active_app"))

        def _parse_active_org(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        active_org = _parse_active_org(d.pop("active_org"))

        orgs = []
        _orgs = d.pop("orgs")
        for orgs_item_data in _orgs:
            orgs_item = MeOrg.from_dict(orgs_item_data)

            orgs.append(orgs_item)

        user = MeUser.from_dict(d.pop("user"))

        me_response = cls(
            active_app=active_app,
            active_org=active_org,
            orgs=orgs,
            user=user,
        )

        me_response.additional_properties = d
        return me_response

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
