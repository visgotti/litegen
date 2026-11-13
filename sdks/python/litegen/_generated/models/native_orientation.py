from typing import Any, Dict, Type, TypeVar

from attrs import define as _attrs_define

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

    def to_dict(self) -> Dict[str, Any]:
        evidence = self.evidence

        front = self.front

        up = self.up

        field_dict: Dict[str, Any] = {}
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

        return native_orientation
