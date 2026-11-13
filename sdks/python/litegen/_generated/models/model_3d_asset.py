import datetime
from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field
from dateutil.parser import isoparse

from ..models.model_3d_asset_kind import Model3DAssetKind
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.scene_manifest import SceneManifest


T = TypeVar("T", bound="Model3DAsset")


@_attrs_define
class Model3DAsset:
    """One file produced by a 3D generation, re-hosted on litegen storage (the
    app's own bucket when it configured one).

    A `completed` generation carries one `mesh` asset for every format in its
    `output_formats` — never fewer; a missing one fails the generation instead —
    plus whatever textures and previews the vendor returned, and, when the
    request set `output_packages`, a `package` asset (a zip) for each of those
    formats that is a file and its sidecars, after every other asset.

        Attributes:
            format_ (str): Lowercase container without a dot. Meshes use the `Model3dFormat`
                vocabulary; textures and previews their image format (`png`, `jpg`,
                `webp`). A plain string so a client tolerates a format added later.
            kind (Model3DAssetKind): What a `Model3dAsset` is.
            url (str): Always absolute: a root-relative path is indistinguishable from an
                outage to a client fetching it from a worker. Meshes live at
                `…/{generation_id}/model.{format}`, packages at
                `…/{generation_id}/package_{format}.zip`.
            expires_at (Union[None, Unset, datetime.datetime]): When a signed link stops working (`docs/storage-access.md`);
                read the
                generation again for a fresh one. Absent on every other URL.
            height (Union[None, Unset, int]): Pixel height, on textures and previews whose vendor reports it.
            kept (Union[None, Unset, bool]): `false` when litegen kept no copy (`keep_outputs: false`): the file is
                the vendor's own, unconverted, and the vendor deletes it on its own
                schedule. Absent when litegen kept it.
            polycount (Union[None, Unset, int]): Triangle count, on meshes whose vendor reports one. A container litegen
                derived carries its source's count — conversion keeps every triangle.
            scene (Union['SceneManifest', None, Unset]):
            sha256 (Union[None, Unset, str]): Lowercase hex SHA-256 of the stored file.
            size_bytes (Union[None, Unset, int]): Size of the stored file.
            width (Union[None, Unset, int]): Pixel width, on textures and previews whose vendor reports it.
    """

    format_: str
    kind: Model3DAssetKind
    url: str
    expires_at: Union[None, Unset, datetime.datetime] = UNSET
    height: Union[None, Unset, int] = UNSET
    kept: Union[None, Unset, bool] = UNSET
    polycount: Union[None, Unset, int] = UNSET
    scene: Union["SceneManifest", None, Unset] = UNSET
    sha256: Union[None, Unset, str] = UNSET
    size_bytes: Union[None, Unset, int] = UNSET
    width: Union[None, Unset, int] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.scene_manifest import SceneManifest

        format_ = self.format_

        kind = self.kind.value

        url = self.url

        expires_at: Union[None, Unset, str]
        if isinstance(self.expires_at, Unset):
            expires_at = UNSET
        elif isinstance(self.expires_at, datetime.datetime):
            expires_at = self.expires_at.isoformat()
        else:
            expires_at = self.expires_at

        height: Union[None, Unset, int]
        if isinstance(self.height, Unset):
            height = UNSET
        else:
            height = self.height

        kept: Union[None, Unset, bool]
        if isinstance(self.kept, Unset):
            kept = UNSET
        else:
            kept = self.kept

        polycount: Union[None, Unset, int]
        if isinstance(self.polycount, Unset):
            polycount = UNSET
        else:
            polycount = self.polycount

        scene: Union[Dict[str, Any], None, Unset]
        if isinstance(self.scene, Unset):
            scene = UNSET
        elif isinstance(self.scene, SceneManifest):
            scene = self.scene.to_dict()
        else:
            scene = self.scene

        sha256: Union[None, Unset, str]
        if isinstance(self.sha256, Unset):
            sha256 = UNSET
        else:
            sha256 = self.sha256

        size_bytes: Union[None, Unset, int]
        if isinstance(self.size_bytes, Unset):
            size_bytes = UNSET
        else:
            size_bytes = self.size_bytes

        width: Union[None, Unset, int]
        if isinstance(self.width, Unset):
            width = UNSET
        else:
            width = self.width

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "format": format_,
                "kind": kind,
                "url": url,
            }
        )
        if expires_at is not UNSET:
            field_dict["expires_at"] = expires_at
        if height is not UNSET:
            field_dict["height"] = height
        if kept is not UNSET:
            field_dict["kept"] = kept
        if polycount is not UNSET:
            field_dict["polycount"] = polycount
        if scene is not UNSET:
            field_dict["scene"] = scene
        if sha256 is not UNSET:
            field_dict["sha256"] = sha256
        if size_bytes is not UNSET:
            field_dict["size_bytes"] = size_bytes
        if width is not UNSET:
            field_dict["width"] = width

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.scene_manifest import SceneManifest

        d = src_dict.copy()
        format_ = d.pop("format")

        kind = Model3DAssetKind(d.pop("kind"))

        url = d.pop("url")

        def _parse_expires_at(data: object) -> Union[None, Unset, datetime.datetime]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                expires_at_type_0 = isoparse(data)

                return expires_at_type_0
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, datetime.datetime], data)

        expires_at = _parse_expires_at(d.pop("expires_at", UNSET))

        def _parse_height(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        height = _parse_height(d.pop("height", UNSET))

        def _parse_kept(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        kept = _parse_kept(d.pop("kept", UNSET))

        def _parse_polycount(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        polycount = _parse_polycount(d.pop("polycount", UNSET))

        def _parse_scene(data: object) -> Union["SceneManifest", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                scene_type_1 = SceneManifest.from_dict(data)

                return scene_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SceneManifest", None, Unset], data)

        scene = _parse_scene(d.pop("scene", UNSET))

        def _parse_sha256(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        sha256 = _parse_sha256(d.pop("sha256", UNSET))

        def _parse_size_bytes(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        size_bytes = _parse_size_bytes(d.pop("size_bytes", UNSET))

        def _parse_width(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        width = _parse_width(d.pop("width", UNSET))

        model_3d_asset = cls(
            format_=format_,
            kind=kind,
            url=url,
            expires_at=expires_at,
            height=height,
            kept=kept,
            polycount=polycount,
            scene=scene,
            sha256=sha256,
            size_bytes=size_bytes,
            width=width,
        )

        model_3d_asset.additional_properties = d
        return model_3d_asset

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
