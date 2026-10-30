from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.scene_animation import SceneAnimation
    from ..models.scene_bounds import SceneBounds
    from ..models.scene_material import SceneMaterial
    from ..models.scene_skin import SceneSkin


T = TypeVar("T", bound="SceneManifest")


@_attrs_define
class SceneManifest:
    """What litegen measured in a delivered mesh (3D doc §3D). Every delivered
    mesh is in glTF's conventions: metres, +Y up, the subject facing +Z with
    its right side at −X.

        Attributes:
            animations (List['SceneAnimation']):
            extensions_used (List[str]): glTF extensions the file uses (none is ever required).
            front (str): `+z`, or `unverified` when the model's orientation has not been proven
                (litegen never reports a guess).
            materials (List['SceneMaterial']):
            oriented_by (str): `vendor` (it arrived so) or `litegen` (litegen turned it).
            triangle_count (int):
            units (str): Always `m`.
            up (str): Always `+y`.
            uv_sets (int):
            vertex_colors (bool):
            vertex_count (int):
            bounds (Union['SceneBounds', None, Unset]):
            skin (Union['SceneSkin', None, Unset]):
    """

    animations: List["SceneAnimation"]
    extensions_used: List[str]
    front: str
    materials: List["SceneMaterial"]
    oriented_by: str
    triangle_count: int
    units: str
    up: str
    uv_sets: int
    vertex_colors: bool
    vertex_count: int
    bounds: Union["SceneBounds", None, Unset] = UNSET
    skin: Union["SceneSkin", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.scene_bounds import SceneBounds
        from ..models.scene_skin import SceneSkin

        animations = []
        for animations_item_data in self.animations:
            animations_item = animations_item_data.to_dict()
            animations.append(animations_item)

        extensions_used = self.extensions_used

        front = self.front

        materials = []
        for materials_item_data in self.materials:
            materials_item = materials_item_data.to_dict()
            materials.append(materials_item)

        oriented_by = self.oriented_by

        triangle_count = self.triangle_count

        units = self.units

        up = self.up

        uv_sets = self.uv_sets

        vertex_colors = self.vertex_colors

        vertex_count = self.vertex_count

        bounds: Union[Dict[str, Any], None, Unset]
        if isinstance(self.bounds, Unset):
            bounds = UNSET
        elif isinstance(self.bounds, SceneBounds):
            bounds = self.bounds.to_dict()
        else:
            bounds = self.bounds

        skin: Union[Dict[str, Any], None, Unset]
        if isinstance(self.skin, Unset):
            skin = UNSET
        elif isinstance(self.skin, SceneSkin):
            skin = self.skin.to_dict()
        else:
            skin = self.skin

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "animations": animations,
                "extensions_used": extensions_used,
                "front": front,
                "materials": materials,
                "oriented_by": oriented_by,
                "triangle_count": triangle_count,
                "units": units,
                "up": up,
                "uv_sets": uv_sets,
                "vertex_colors": vertex_colors,
                "vertex_count": vertex_count,
            }
        )
        if bounds is not UNSET:
            field_dict["bounds"] = bounds
        if skin is not UNSET:
            field_dict["skin"] = skin

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.scene_animation import SceneAnimation
        from ..models.scene_bounds import SceneBounds
        from ..models.scene_material import SceneMaterial
        from ..models.scene_skin import SceneSkin

        d = src_dict.copy()
        animations = []
        _animations = d.pop("animations")
        for animations_item_data in _animations:
            animations_item = SceneAnimation.from_dict(animations_item_data)

            animations.append(animations_item)

        extensions_used = cast(List[str], d.pop("extensions_used"))

        front = d.pop("front")

        materials = []
        _materials = d.pop("materials")
        for materials_item_data in _materials:
            materials_item = SceneMaterial.from_dict(materials_item_data)

            materials.append(materials_item)

        oriented_by = d.pop("oriented_by")

        triangle_count = d.pop("triangle_count")

        units = d.pop("units")

        up = d.pop("up")

        uv_sets = d.pop("uv_sets")

        vertex_colors = d.pop("vertex_colors")

        vertex_count = d.pop("vertex_count")

        def _parse_bounds(data: object) -> Union["SceneBounds", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                bounds_type_1 = SceneBounds.from_dict(data)

                return bounds_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SceneBounds", None, Unset], data)

        bounds = _parse_bounds(d.pop("bounds", UNSET))

        def _parse_skin(data: object) -> Union["SceneSkin", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                skin_type_1 = SceneSkin.from_dict(data)

                return skin_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SceneSkin", None, Unset], data)

        skin = _parse_skin(d.pop("skin", UNSET))

        scene_manifest = cls(
            animations=animations,
            extensions_used=extensions_used,
            front=front,
            materials=materials,
            oriented_by=oriented_by,
            triangle_count=triangle_count,
            units=units,
            up=up,
            uv_sets=uv_sets,
            vertex_colors=vertex_colors,
            vertex_count=vertex_count,
            bounds=bounds,
            skin=skin,
        )

        scene_manifest.additional_properties = d
        return scene_manifest

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
