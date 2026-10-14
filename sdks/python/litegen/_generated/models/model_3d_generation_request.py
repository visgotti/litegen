from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.model_3d_format import Model3DFormat
from ..models.model_3d_generation_request_output_format import (
    Model3DGenerationRequestOutputFormat,
)
from ..models.model_3d_symmetry import Model3DSymmetry
from ..models.model_3d_topology import Model3DTopology
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.reference_image import ReferenceImage


T = TypeVar("T", bound="Model3DGenerationRequest")


@_attrs_define
class Model3DGenerationRequest:
    """Request body for `POST /v1/models3d/generations` and `POST /v1/models3d/cost`.

    The mode is inferred, never passed: no `reference_images` is text-to-3D, an
    `init` image is image-to-3D, and `view-*` images are multiview-to-3D.

    Every 3D param is optional and model-dependent; `GET /v1/models/{id}` →
    `params` says which a model takes. One it does not take is refused with
    `param_unsupported` when `strict` is true (the default), and dropped — and
    named in the `X-Litegen-Dropped-Params` response header — when `strict` is
    false. A supported param with a bad VALUE is refused in both modes.

        Attributes:
            model (str):
            extra (Union[Unset, Any]):
            metadata (Union[Unset, Any]):
            n (Union[Unset, int]):
            negative_prompt (Union[None, Unset, str]):
            prompt (Union[Unset, str]): Whether a prompt is required is decided per model, by its
                `prompt.required` in `GET /v1/models/{id}` — never by the wire format.
                Omitting it is the same as sending `""`, which an image-to-3D model
                with no prompt field (TRELLIS, Hunyuan3D v2) accepts and a text model
                answers with `prompt_required`.
            reference_images (Union[Unset, List['ReferenceImage']]):
            seed (Union[None, Unset, int]):
            strict (Union[Unset, bool]):
            output_format (Union[Unset, Model3DGenerationRequestOutputFormat]): Superseded by `output_formats`. A lone value
                is folded into it; when both are sent, `output_formats` wins and this one is reported dropped.
            output_formats (Union[List[Model3DFormat], None, Unset]): Mesh containers to deliver, e.g. `["glb", "fbx"]`. A
                promise, not a
                hint: every format named here is present on a `completed` generation,
                or the generation ends `failed`. Omitted means the model's declared
                default, and `["glb"]` when it declares none. Case-insensitive;
                duplicates collapse.

                Any set of the containers litegen derives (the model's served
                `derivable_values`) costs one vendor job between them; every other
                container costs one job of its own. A request needing more vendor jobs
                than the model's `max_items` is refused with `param_too_many`.
            pbr (Union[None, Unset, bool]): PBR materials. `false` asks for flat shading, not "no material".
            rig (Union[None, Unset, bool]): Auto-rig: deliver the mesh with a skeleton.
            symmetry (Union[Model3DSymmetry, None, Unset]):
            target_polycount (Union[None, Unset, int]): Approximate triangle budget for the mesh, within the model's
                declared
                `target_polycount` range.
            texture (Union[None, Unset, bool]): Texture the mesh, rather than returning bare geometry.
            topology (Union[Model3DTopology, None, Unset]):
    """

    model: str
    extra: Union[Unset, Any] = UNSET
    metadata: Union[Unset, Any] = UNSET
    n: Union[Unset, int] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    prompt: Union[Unset, str] = UNSET
    reference_images: Union[Unset, List["ReferenceImage"]] = UNSET
    seed: Union[None, Unset, int] = UNSET
    strict: Union[Unset, bool] = UNSET
    output_format: Union[Unset, Model3DGenerationRequestOutputFormat] = UNSET
    output_formats: Union[List[Model3DFormat], None, Unset] = UNSET
    pbr: Union[None, Unset, bool] = UNSET
    rig: Union[None, Unset, bool] = UNSET
    symmetry: Union[Model3DSymmetry, None, Unset] = UNSET
    target_polycount: Union[None, Unset, int] = UNSET
    texture: Union[None, Unset, bool] = UNSET
    topology: Union[Model3DTopology, None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        model = self.model

        extra = self.extra

        metadata = self.metadata

        n = self.n

        negative_prompt: Union[None, Unset, str]
        if isinstance(self.negative_prompt, Unset):
            negative_prompt = UNSET
        else:
            negative_prompt = self.negative_prompt

        prompt = self.prompt

        reference_images: Union[Unset, List[Dict[str, Any]]] = UNSET
        if not isinstance(self.reference_images, Unset):
            reference_images = []
            for reference_images_item_data in self.reference_images:
                reference_images_item = reference_images_item_data.to_dict()
                reference_images.append(reference_images_item)

        seed: Union[None, Unset, int]
        if isinstance(self.seed, Unset):
            seed = UNSET
        else:
            seed = self.seed

        strict = self.strict

        output_format: Union[Unset, str] = UNSET
        if not isinstance(self.output_format, Unset):
            output_format = self.output_format.value

        output_formats: Union[List[str], None, Unset]
        if isinstance(self.output_formats, Unset):
            output_formats = UNSET
        elif isinstance(self.output_formats, list):
            output_formats = []
            for output_formats_type_0_item_data in self.output_formats:
                output_formats_type_0_item = output_formats_type_0_item_data.value
                output_formats.append(output_formats_type_0_item)

        else:
            output_formats = self.output_formats

        pbr: Union[None, Unset, bool]
        if isinstance(self.pbr, Unset):
            pbr = UNSET
        else:
            pbr = self.pbr

        rig: Union[None, Unset, bool]
        if isinstance(self.rig, Unset):
            rig = UNSET
        else:
            rig = self.rig

        symmetry: Union[None, Unset, str]
        if isinstance(self.symmetry, Unset):
            symmetry = UNSET
        elif isinstance(self.symmetry, Model3DSymmetry):
            symmetry = self.symmetry.value
        else:
            symmetry = self.symmetry

        target_polycount: Union[None, Unset, int]
        if isinstance(self.target_polycount, Unset):
            target_polycount = UNSET
        else:
            target_polycount = self.target_polycount

        texture: Union[None, Unset, bool]
        if isinstance(self.texture, Unset):
            texture = UNSET
        else:
            texture = self.texture

        topology: Union[None, Unset, str]
        if isinstance(self.topology, Unset):
            topology = UNSET
        elif isinstance(self.topology, Model3DTopology):
            topology = self.topology.value
        else:
            topology = self.topology

        field_dict: Dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "model": model,
            }
        )
        if extra is not UNSET:
            field_dict["extra"] = extra
        if metadata is not UNSET:
            field_dict["metadata"] = metadata
        if n is not UNSET:
            field_dict["n"] = n
        if negative_prompt is not UNSET:
            field_dict["negative_prompt"] = negative_prompt
        if prompt is not UNSET:
            field_dict["prompt"] = prompt
        if reference_images is not UNSET:
            field_dict["reference_images"] = reference_images
        if seed is not UNSET:
            field_dict["seed"] = seed
        if strict is not UNSET:
            field_dict["strict"] = strict
        if output_format is not UNSET:
            field_dict["output_format"] = output_format
        if output_formats is not UNSET:
            field_dict["output_formats"] = output_formats
        if pbr is not UNSET:
            field_dict["pbr"] = pbr
        if rig is not UNSET:
            field_dict["rig"] = rig
        if symmetry is not UNSET:
            field_dict["symmetry"] = symmetry
        if target_polycount is not UNSET:
            field_dict["target_polycount"] = target_polycount
        if texture is not UNSET:
            field_dict["texture"] = texture
        if topology is not UNSET:
            field_dict["topology"] = topology

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.reference_image import ReferenceImage

        d = src_dict.copy()
        model = d.pop("model")

        extra = d.pop("extra", UNSET)

        metadata = d.pop("metadata", UNSET)

        n = d.pop("n", UNSET)

        def _parse_negative_prompt(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        negative_prompt = _parse_negative_prompt(d.pop("negative_prompt", UNSET))

        prompt = d.pop("prompt", UNSET)

        reference_images = []
        _reference_images = d.pop("reference_images", UNSET)
        for reference_images_item_data in _reference_images or []:
            reference_images_item = ReferenceImage.from_dict(reference_images_item_data)

            reference_images.append(reference_images_item)

        def _parse_seed(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        seed = _parse_seed(d.pop("seed", UNSET))

        strict = d.pop("strict", UNSET)

        _output_format = d.pop("output_format", UNSET)
        output_format: Union[Unset, Model3DGenerationRequestOutputFormat]
        if isinstance(_output_format, Unset):
            output_format = UNSET
        else:
            output_format = Model3DGenerationRequestOutputFormat(_output_format)

        def _parse_output_formats(
            data: object,
        ) -> Union[List[Model3DFormat], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                output_formats_type_0 = []
                _output_formats_type_0 = data
                for output_formats_type_0_item_data in _output_formats_type_0:
                    output_formats_type_0_item = Model3DFormat(
                        output_formats_type_0_item_data
                    )

                    output_formats_type_0.append(output_formats_type_0_item)

                return output_formats_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List[Model3DFormat], None, Unset], data)

        output_formats = _parse_output_formats(d.pop("output_formats", UNSET))

        def _parse_pbr(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        pbr = _parse_pbr(d.pop("pbr", UNSET))

        def _parse_rig(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        rig = _parse_rig(d.pop("rig", UNSET))

        def _parse_symmetry(data: object) -> Union[Model3DSymmetry, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                symmetry_type_1 = Model3DSymmetry(data)

                return symmetry_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DSymmetry, None, Unset], data)

        symmetry = _parse_symmetry(d.pop("symmetry", UNSET))

        def _parse_target_polycount(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        target_polycount = _parse_target_polycount(d.pop("target_polycount", UNSET))

        def _parse_texture(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        texture = _parse_texture(d.pop("texture", UNSET))

        def _parse_topology(data: object) -> Union[Model3DTopology, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                topology_type_1 = Model3DTopology(data)

                return topology_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DTopology, None, Unset], data)

        topology = _parse_topology(d.pop("topology", UNSET))

        model_3d_generation_request = cls(
            model=model,
            extra=extra,
            metadata=metadata,
            n=n,
            negative_prompt=negative_prompt,
            prompt=prompt,
            reference_images=reference_images,
            seed=seed,
            strict=strict,
            output_format=output_format,
            output_formats=output_formats,
            pbr=pbr,
            rig=rig,
            symmetry=symmetry,
            target_polycount=target_polycount,
            texture=texture,
            topology=topology,
        )

        model_3d_generation_request.additional_properties = d
        return model_3d_generation_request

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
