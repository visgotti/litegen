from typing import (
    TYPE_CHECKING,
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

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.scene_skin_humanoid_type_0 import SceneSkinHumanoidType0


T = TypeVar("T", bound="SceneSkin")


@_attrs_define
class SceneSkin:
    """
    Attributes:
        bind_pose (str): `t`, `a` or `unknown`, from the upper-arm angle at rest.
        joint_count (int):
        joints (List[str]): Joint names, unique, in skin order.
        humanoid (Union['SceneSkinHumanoidType0', None, Unset]): VRM 1.0 humanoid bone → joint name, for a rig litegen
            recognises;
            `null` for one it does not (never a guess).
        root (Union[None, Unset, str]):
        skeleton (Union[None, Unset, str]): `humanoid` when a known naming scheme matched, else what the request
            asked the rig to be, else absent.
    """

    bind_pose: str
    joint_count: int
    joints: List[str]
    humanoid: Union["SceneSkinHumanoidType0", None, Unset] = UNSET
    root: Union[None, Unset, str] = UNSET
    skeleton: Union[None, Unset, str] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.scene_skin_humanoid_type_0 import SceneSkinHumanoidType0

        bind_pose = self.bind_pose

        joint_count = self.joint_count

        joints = self.joints

        humanoid: Union[Dict[str, Any], None, Unset]
        if isinstance(self.humanoid, Unset):
            humanoid = UNSET
        elif isinstance(self.humanoid, SceneSkinHumanoidType0):
            humanoid = self.humanoid.to_dict()
        else:
            humanoid = self.humanoid

        root: Union[None, Unset, str]
        if isinstance(self.root, Unset):
            root = UNSET
        else:
            root = self.root

        skeleton: Union[None, Unset, str]
        if isinstance(self.skeleton, Unset):
            skeleton = UNSET
        else:
            skeleton = self.skeleton

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "bind_pose": bind_pose,
                "joint_count": joint_count,
                "joints": joints,
            }
        )
        if humanoid is not UNSET:
            field_dict["humanoid"] = humanoid
        if root is not UNSET:
            field_dict["root"] = root
        if skeleton is not UNSET:
            field_dict["skeleton"] = skeleton

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.scene_skin_humanoid_type_0 import SceneSkinHumanoidType0

        d = src_dict.copy()
        bind_pose = d.pop("bind_pose")

        joint_count = d.pop("joint_count")

        joints = cast(List[str], d.pop("joints"))

        def _parse_humanoid(
            data: object,
        ) -> Union["SceneSkinHumanoidType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                humanoid_type_0 = SceneSkinHumanoidType0.from_dict(data)

                return humanoid_type_0
            except:  # noqa: E722
                pass
            return cast(Union["SceneSkinHumanoidType0", None, Unset], data)

        humanoid = _parse_humanoid(d.pop("humanoid", UNSET))

        def _parse_root(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        root = _parse_root(d.pop("root", UNSET))

        def _parse_skeleton(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        skeleton = _parse_skeleton(d.pop("skeleton", UNSET))

        scene_skin = cls(
            bind_pose=bind_pose,
            joint_count=joint_count,
            joints=joints,
            humanoid=humanoid,
            root=root,
            skeleton=skeleton,
        )

        scene_skin.additional_properties = d
        return scene_skin

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
