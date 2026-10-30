from typing import Any, Dict, List, Type, TypeVar

from attrs import define as _attrs_define
from attrs import field as _attrs_field

T = TypeVar("T", bound="NativeOrientation")


@_attrs_define
class NativeOrientation:
    """The axes a 3D model's vendor builds its meshes along (3D doc §2 rule 3).
    Where they differ from glTF's (+Y up, +Z front), litegen adds a root
    rotation and the manifest says `oriented_by: "litegen"`. An operation that
    hands back its source's own geometry says `source` for both: its output
    faces wherever the source did.

        Attributes:
            evidence (str): What showed it: the run, sample file or page.
            front (str): The axis the asset's front faces, at right angles to `up`; `source`
                with `up`.
            up (str): `+x`, `-x`, `+y`, `-y`, `+z` or `-z`; `source` on an operation that
                keeps its source's axes.
    """

    evidence: str
    front: str
    up: str
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        evidence = self.evidence

        front = self.front

        up = self.up

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "evidence": evidence,
                "front": front,
                "up": up,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        d = src_dict.copy()
        evidence = d.pop("evidence")

        front = d.pop("front")

        up = d.pop("up")

        native_orientation = cls(
            evidence=evidence,
            front=front,
            up=up,
        )

        native_orientation.additional_properties = d
        return native_orientation

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
