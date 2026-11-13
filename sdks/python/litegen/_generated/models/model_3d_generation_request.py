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

from ..models.model_3d_dae_morphs import Model3DDaeMorphs
from ..models.model_3d_fbx_color_space import Model3DFbxColorSpace
from ..models.model_3d_format import Model3DFormat
from ..models.model_3d_format_source import Model3DFormatSource
from ..models.model_3d_generation_request_output_format import (
    Model3DGenerationRequestOutputFormat,
)
from ..models.model_3d_joint_naming import Model3DJointNaming
from ..models.model_3d_ktx_2_textures import Model3DKtx2Textures
from ..models.model_3d_light_units import Model3DLightUnits
from ..models.model_3d_skeleton import Model3DSkeleton
from ..models.model_3d_symmetry import Model3DSymmetry
from ..models.model_3d_topology import Model3DTopology
from ..models.model_3d_usd_clips import Model3DUsdClips
from ..models.model_3d_usdz_root import Model3DUsdzRoot
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.animation_request import AnimationRequest
    from ..models.base_generation_request_metadata_type_0 import (
        BaseGenerationRequestMetadataType0,
    )
    from ..models.reference_image import ReferenceImage
    from ..models.source_ref import SourceRef


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
            extra (Union[Unset, Any]): Vendor fields with no canonical param, sent to the vendor as given:
                only the keys in the model's `extra_allowlist` (`GET /v1/models/{id}`),
                each of the type its `extra_params` entry gives, when it has one (a
                wrong value is refused in either mode, with `param: "extra.<key>"`).
                Strict mode refuses any other key (`extra_key_unsupported`); `strict:
                false` drops it and names it in `X-Litegen-Dropped-Params`. Where a
                job's result is sent or stored, and whether the vendor publishes it,
                are litegen's: it polls every job and fires your key's webhook.
            metadata (Union['BaseGenerationRequestMetadataType0', None, Unset]): Your own labels for the job (PixelGenie
                L17): a map of string values,
                keys of letters, digits, `_`, `.` and `-`, at most 1 KB as JSON. Kept on
                the generation, echoed on every answer and webhook, and filters
                `GET /v1/logs` and `GET /v1/stats` as `metadata.<key>=<value>`.
            n (Union[Unset, int]): One image, video or mesh per request: more than 1 is refused
                (`param_out_of_range`); send several requests for several. 0 is the
                same as 1.
            negative_prompt (Union[None, Unset, str]):
            prompt (Union[Unset, str]): Whether a prompt is required is decided per model, by its
                `prompt.required` in `GET /v1/models/{id}` — never by the wire format.
                Omitting it is the same as sending `""`, which an image-to-3D model
                with no prompt field (TRELLIS, Hunyuan3D v2) accepts and a text model
                answers with `prompt_required`.
            reference_images (Union[Unset, List['ReferenceImage']]):
            seed (Union[None, Unset, int]):
            strict (Union[Unset, bool]): `true` (the default) refuses a field the request has no such name for,
                and a param the model does not take, with `param_unsupported`; `false`
                drops both and names them in `X-Litegen-Dropped-Params`. A field inside
                one of the request's objects is named by its path, such as
                `reference_images[0].weight`. A bad value for a param the model does
                take is refused either way. A top-level field the model passes through
                (its `extra_allowlist` in `GET /v1/models/{id}`, such as OpenAI's
                `user`) is sent on as if it were in `extra`, in both modes.
            animations (Union[List['AnimationRequest'], None, Unset]): The clips to make, in order: one glTF animation per
                entry, named its
                `name`, or the name litegen gives a clip that leaves it out.
            center (Union[None, Unset, bool]): Centre the mesh's bounds on x and z (3D doc §3D). litegen applies it
                on delivery, to any model's mesh.
            dae_morphs (Union[Model3DDaeMorphs, None, Unset]):
            fbx_color_space (Union[Model3DFbxColorSpace, None, Unset]):
            fbx_light_units (Union[Model3DLightUnits, None, Unset]):
            format_source (Union[Model3DFormatSource, None, Unset]):
            fps (Union[None, Unset, int]): Frames per second of the delivered clips.
            ground (Union[None, Unset, bool]): Put the mesh's lowest point at y = 0 (3D doc §3D). litegen applies it
                on delivery, to any model's mesh.
            height_m (Union[None, Unset, float]): The delivered mesh's height in metres (3D doc §3D): litegen scales
                any mesh it delivers to it, and a rig model whose vendor takes a
                height gets it too. At most 10,000 m, or within a model's own
                `height_m` range.
            joint_naming (Union[Model3DJointNaming, None, Unset]):
            keep_uvs (Union[None, Unset, bool]): Paint the source's own UV layout rather than a new one.
            ktx2_textures (Union[Model3DKtx2Textures, None, Unset]):
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
            output_packages (Union[None, Unset, bool]): Also deliver each requested container that is a file and its
                sidecars (an `obj` with its `.mtl` and textures, a `dae`, `usda` or
                `usdc` with its textures, a `gltf` with its `.bin` and images) as one zip of
                them: an asset of kind `package` whose `format` is the container
                inside. litegen applies it on delivery, to any model's mesh, and the
                `mesh` assets stay as they are. A container that is one file for this
                mesh gets no package. A request that keeps nothing (`keep_outputs:
                false`) can't ask for packages: `option_needs_keeping`.
            pbr (Union[None, Unset, bool]): PBR materials. `false` asks for flat shading, not "no material".
            rig (Union[None, Unset, bool]): Auto-rig: deliver the mesh with a skeleton.
            skeleton (Union[Model3DSkeleton, None, Unset]):
            source_model (Union['SourceRef', None, Unset]):
            symmetry (Union[Model3DSymmetry, None, Unset]):
            target_polycount (Union[None, Unset, int]): Approximate triangle budget for the mesh, within the model's
                declared
                `target_polycount` range.
            texture (Union[None, Unset, bool]): Texture the mesh, rather than returning bare geometry.
            texture_resolution (Union[None, Unset, int]): A `texture` model's map size in pixels, one of its
                `texture_resolutions`.
            topology (Union[Model3DTopology, None, Unset]):
            usd_clips (Union[Model3DUsdClips, None, Unset]):
            usd_light_units (Union[Model3DLightUnits, None, Unset]):
            usdz_root (Union[Model3DUsdzRoot, None, Unset]):
    """

    model: str
    extra: Union[Unset, Any] = UNSET
    metadata: Union["BaseGenerationRequestMetadataType0", None, Unset] = UNSET
    n: Union[Unset, int] = UNSET
    negative_prompt: Union[None, Unset, str] = UNSET
    prompt: Union[Unset, str] = UNSET
    reference_images: Union[Unset, List["ReferenceImage"]] = UNSET
    seed: Union[None, Unset, int] = UNSET
    strict: Union[Unset, bool] = UNSET
    animations: Union[List["AnimationRequest"], None, Unset] = UNSET
    center: Union[None, Unset, bool] = UNSET
    dae_morphs: Union[Model3DDaeMorphs, None, Unset] = UNSET
    fbx_color_space: Union[Model3DFbxColorSpace, None, Unset] = UNSET
    fbx_light_units: Union[Model3DLightUnits, None, Unset] = UNSET
    format_source: Union[Model3DFormatSource, None, Unset] = UNSET
    fps: Union[None, Unset, int] = UNSET
    ground: Union[None, Unset, bool] = UNSET
    height_m: Union[None, Unset, float] = UNSET
    joint_naming: Union[Model3DJointNaming, None, Unset] = UNSET
    keep_uvs: Union[None, Unset, bool] = UNSET
    ktx2_textures: Union[Model3DKtx2Textures, None, Unset] = UNSET
    output_format: Union[Unset, Model3DGenerationRequestOutputFormat] = UNSET
    output_formats: Union[List[Model3DFormat], None, Unset] = UNSET
    output_packages: Union[None, Unset, bool] = UNSET
    pbr: Union[None, Unset, bool] = UNSET
    rig: Union[None, Unset, bool] = UNSET
    skeleton: Union[Model3DSkeleton, None, Unset] = UNSET
    source_model: Union["SourceRef", None, Unset] = UNSET
    symmetry: Union[Model3DSymmetry, None, Unset] = UNSET
    target_polycount: Union[None, Unset, int] = UNSET
    texture: Union[None, Unset, bool] = UNSET
    texture_resolution: Union[None, Unset, int] = UNSET
    topology: Union[Model3DTopology, None, Unset] = UNSET
    usd_clips: Union[Model3DUsdClips, None, Unset] = UNSET
    usd_light_units: Union[Model3DLightUnits, None, Unset] = UNSET
    usdz_root: Union[Model3DUsdzRoot, None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.source_ref import SourceRef

        model = self.model

        extra = self.extra

        metadata: Union[Dict[str, Any], None, Unset]
        if isinstance(self.metadata, Unset):
            metadata = UNSET
        elif isinstance(self.metadata, BaseGenerationRequestMetadataType0):
            metadata = self.metadata.to_dict()
        else:
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

        animations: Union[List[Dict[str, Any]], None, Unset]
        if isinstance(self.animations, Unset):
            animations = UNSET
        elif isinstance(self.animations, list):
            animations = []
            for animations_type_0_item_data in self.animations:
                animations_type_0_item = animations_type_0_item_data.to_dict()
                animations.append(animations_type_0_item)

        else:
            animations = self.animations

        center: Union[None, Unset, bool]
        if isinstance(self.center, Unset):
            center = UNSET
        else:
            center = self.center

        dae_morphs: Union[None, Unset, str]
        if isinstance(self.dae_morphs, Unset):
            dae_morphs = UNSET
        elif isinstance(self.dae_morphs, Model3DDaeMorphs):
            dae_morphs = self.dae_morphs.value
        else:
            dae_morphs = self.dae_morphs

        fbx_color_space: Union[None, Unset, str]
        if isinstance(self.fbx_color_space, Unset):
            fbx_color_space = UNSET
        elif isinstance(self.fbx_color_space, Model3DFbxColorSpace):
            fbx_color_space = self.fbx_color_space.value
        else:
            fbx_color_space = self.fbx_color_space

        fbx_light_units: Union[None, Unset, str]
        if isinstance(self.fbx_light_units, Unset):
            fbx_light_units = UNSET
        elif isinstance(self.fbx_light_units, Model3DLightUnits):
            fbx_light_units = self.fbx_light_units.value
        else:
            fbx_light_units = self.fbx_light_units

        format_source: Union[None, Unset, str]
        if isinstance(self.format_source, Unset):
            format_source = UNSET
        elif isinstance(self.format_source, Model3DFormatSource):
            format_source = self.format_source.value
        else:
            format_source = self.format_source

        fps: Union[None, Unset, int]
        if isinstance(self.fps, Unset):
            fps = UNSET
        else:
            fps = self.fps

        ground: Union[None, Unset, bool]
        if isinstance(self.ground, Unset):
            ground = UNSET
        else:
            ground = self.ground

        height_m: Union[None, Unset, float]
        if isinstance(self.height_m, Unset):
            height_m = UNSET
        else:
            height_m = self.height_m

        joint_naming: Union[None, Unset, str]
        if isinstance(self.joint_naming, Unset):
            joint_naming = UNSET
        elif isinstance(self.joint_naming, Model3DJointNaming):
            joint_naming = self.joint_naming.value
        else:
            joint_naming = self.joint_naming

        keep_uvs: Union[None, Unset, bool]
        if isinstance(self.keep_uvs, Unset):
            keep_uvs = UNSET
        else:
            keep_uvs = self.keep_uvs

        ktx2_textures: Union[None, Unset, str]
        if isinstance(self.ktx2_textures, Unset):
            ktx2_textures = UNSET
        elif isinstance(self.ktx2_textures, Model3DKtx2Textures):
            ktx2_textures = self.ktx2_textures.value
        else:
            ktx2_textures = self.ktx2_textures

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

        output_packages: Union[None, Unset, bool]
        if isinstance(self.output_packages, Unset):
            output_packages = UNSET
        else:
            output_packages = self.output_packages

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

        skeleton: Union[None, Unset, str]
        if isinstance(self.skeleton, Unset):
            skeleton = UNSET
        elif isinstance(self.skeleton, Model3DSkeleton):
            skeleton = self.skeleton.value
        else:
            skeleton = self.skeleton

        source_model: Union[Dict[str, Any], None, Unset]
        if isinstance(self.source_model, Unset):
            source_model = UNSET
        elif isinstance(self.source_model, SourceRef):
            source_model = self.source_model.to_dict()
        else:
            source_model = self.source_model

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

        texture_resolution: Union[None, Unset, int]
        if isinstance(self.texture_resolution, Unset):
            texture_resolution = UNSET
        else:
            texture_resolution = self.texture_resolution

        topology: Union[None, Unset, str]
        if isinstance(self.topology, Unset):
            topology = UNSET
        elif isinstance(self.topology, Model3DTopology):
            topology = self.topology.value
        else:
            topology = self.topology

        usd_clips: Union[None, Unset, str]
        if isinstance(self.usd_clips, Unset):
            usd_clips = UNSET
        elif isinstance(self.usd_clips, Model3DUsdClips):
            usd_clips = self.usd_clips.value
        else:
            usd_clips = self.usd_clips

        usd_light_units: Union[None, Unset, str]
        if isinstance(self.usd_light_units, Unset):
            usd_light_units = UNSET
        elif isinstance(self.usd_light_units, Model3DLightUnits):
            usd_light_units = self.usd_light_units.value
        else:
            usd_light_units = self.usd_light_units

        usdz_root: Union[None, Unset, str]
        if isinstance(self.usdz_root, Unset):
            usdz_root = UNSET
        elif isinstance(self.usdz_root, Model3DUsdzRoot):
            usdz_root = self.usdz_root.value
        else:
            usdz_root = self.usdz_root

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
        if animations is not UNSET:
            field_dict["animations"] = animations
        if center is not UNSET:
            field_dict["center"] = center
        if dae_morphs is not UNSET:
            field_dict["dae_morphs"] = dae_morphs
        if fbx_color_space is not UNSET:
            field_dict["fbx_color_space"] = fbx_color_space
        if fbx_light_units is not UNSET:
            field_dict["fbx_light_units"] = fbx_light_units
        if format_source is not UNSET:
            field_dict["format_source"] = format_source
        if fps is not UNSET:
            field_dict["fps"] = fps
        if ground is not UNSET:
            field_dict["ground"] = ground
        if height_m is not UNSET:
            field_dict["height_m"] = height_m
        if joint_naming is not UNSET:
            field_dict["joint_naming"] = joint_naming
        if keep_uvs is not UNSET:
            field_dict["keep_uvs"] = keep_uvs
        if ktx2_textures is not UNSET:
            field_dict["ktx2_textures"] = ktx2_textures
        if output_format is not UNSET:
            field_dict["output_format"] = output_format
        if output_formats is not UNSET:
            field_dict["output_formats"] = output_formats
        if output_packages is not UNSET:
            field_dict["output_packages"] = output_packages
        if pbr is not UNSET:
            field_dict["pbr"] = pbr
        if rig is not UNSET:
            field_dict["rig"] = rig
        if skeleton is not UNSET:
            field_dict["skeleton"] = skeleton
        if source_model is not UNSET:
            field_dict["source_model"] = source_model
        if symmetry is not UNSET:
            field_dict["symmetry"] = symmetry
        if target_polycount is not UNSET:
            field_dict["target_polycount"] = target_polycount
        if texture is not UNSET:
            field_dict["texture"] = texture
        if texture_resolution is not UNSET:
            field_dict["texture_resolution"] = texture_resolution
        if topology is not UNSET:
            field_dict["topology"] = topology
        if usd_clips is not UNSET:
            field_dict["usd_clips"] = usd_clips
        if usd_light_units is not UNSET:
            field_dict["usd_light_units"] = usd_light_units
        if usdz_root is not UNSET:
            field_dict["usdz_root"] = usdz_root

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.animation_request import AnimationRequest
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.reference_image import ReferenceImage
        from ..models.source_ref import SourceRef

        d = src_dict.copy()
        model = d.pop("model")

        extra = d.pop("extra", UNSET)

        def _parse_metadata(
            data: object,
        ) -> Union["BaseGenerationRequestMetadataType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                metadata_type_0 = BaseGenerationRequestMetadataType0.from_dict(data)

                return metadata_type_0
            except:  # noqa: E722
                pass
            return cast(Union["BaseGenerationRequestMetadataType0", None, Unset], data)

        metadata = _parse_metadata(d.pop("metadata", UNSET))

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

        def _parse_animations(
            data: object,
        ) -> Union[List["AnimationRequest"], None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, list):
                    raise TypeError()
                animations_type_0 = []
                _animations_type_0 = data
                for animations_type_0_item_data in _animations_type_0:
                    animations_type_0_item = AnimationRequest.from_dict(
                        animations_type_0_item_data
                    )

                    animations_type_0.append(animations_type_0_item)

                return animations_type_0
            except:  # noqa: E722
                pass
            return cast(Union[List["AnimationRequest"], None, Unset], data)

        animations = _parse_animations(d.pop("animations", UNSET))

        def _parse_center(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        center = _parse_center(d.pop("center", UNSET))

        def _parse_dae_morphs(data: object) -> Union[Model3DDaeMorphs, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                dae_morphs_type_1 = Model3DDaeMorphs(data)

                return dae_morphs_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DDaeMorphs, None, Unset], data)

        dae_morphs = _parse_dae_morphs(d.pop("dae_morphs", UNSET))

        def _parse_fbx_color_space(
            data: object,
        ) -> Union[Model3DFbxColorSpace, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                fbx_color_space_type_1 = Model3DFbxColorSpace(data)

                return fbx_color_space_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DFbxColorSpace, None, Unset], data)

        fbx_color_space = _parse_fbx_color_space(d.pop("fbx_color_space", UNSET))

        def _parse_fbx_light_units(
            data: object,
        ) -> Union[Model3DLightUnits, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                fbx_light_units_type_1 = Model3DLightUnits(data)

                return fbx_light_units_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DLightUnits, None, Unset], data)

        fbx_light_units = _parse_fbx_light_units(d.pop("fbx_light_units", UNSET))

        def _parse_format_source(
            data: object,
        ) -> Union[Model3DFormatSource, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                format_source_type_1 = Model3DFormatSource(data)

                return format_source_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DFormatSource, None, Unset], data)

        format_source = _parse_format_source(d.pop("format_source", UNSET))

        def _parse_fps(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        fps = _parse_fps(d.pop("fps", UNSET))

        def _parse_ground(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        ground = _parse_ground(d.pop("ground", UNSET))

        def _parse_height_m(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        height_m = _parse_height_m(d.pop("height_m", UNSET))

        def _parse_joint_naming(data: object) -> Union[Model3DJointNaming, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                joint_naming_type_1 = Model3DJointNaming(data)

                return joint_naming_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DJointNaming, None, Unset], data)

        joint_naming = _parse_joint_naming(d.pop("joint_naming", UNSET))

        def _parse_keep_uvs(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        keep_uvs = _parse_keep_uvs(d.pop("keep_uvs", UNSET))

        def _parse_ktx2_textures(
            data: object,
        ) -> Union[Model3DKtx2Textures, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                ktx2_textures_type_1 = Model3DKtx2Textures(data)

                return ktx2_textures_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DKtx2Textures, None, Unset], data)

        ktx2_textures = _parse_ktx2_textures(d.pop("ktx2_textures", UNSET))

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

        def _parse_output_packages(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        output_packages = _parse_output_packages(d.pop("output_packages", UNSET))

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

        def _parse_skeleton(data: object) -> Union[Model3DSkeleton, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                skeleton_type_1 = Model3DSkeleton(data)

                return skeleton_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DSkeleton, None, Unset], data)

        skeleton = _parse_skeleton(d.pop("skeleton", UNSET))

        def _parse_source_model(data: object) -> Union["SourceRef", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                source_model_type_1 = SourceRef.from_dict(data)

                return source_model_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SourceRef", None, Unset], data)

        source_model = _parse_source_model(d.pop("source_model", UNSET))

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

        def _parse_texture_resolution(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        texture_resolution = _parse_texture_resolution(
            d.pop("texture_resolution", UNSET)
        )

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

        def _parse_usd_clips(data: object) -> Union[Model3DUsdClips, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                usd_clips_type_1 = Model3DUsdClips(data)

                return usd_clips_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DUsdClips, None, Unset], data)

        usd_clips = _parse_usd_clips(d.pop("usd_clips", UNSET))

        def _parse_usd_light_units(
            data: object,
        ) -> Union[Model3DLightUnits, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                usd_light_units_type_1 = Model3DLightUnits(data)

                return usd_light_units_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DLightUnits, None, Unset], data)

        usd_light_units = _parse_usd_light_units(d.pop("usd_light_units", UNSET))

        def _parse_usdz_root(data: object) -> Union[Model3DUsdzRoot, None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                usdz_root_type_1 = Model3DUsdzRoot(data)

                return usdz_root_type_1
            except:  # noqa: E722
                pass
            return cast(Union[Model3DUsdzRoot, None, Unset], data)

        usdz_root = _parse_usdz_root(d.pop("usdz_root", UNSET))

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
            animations=animations,
            center=center,
            dae_morphs=dae_morphs,
            fbx_color_space=fbx_color_space,
            fbx_light_units=fbx_light_units,
            format_source=format_source,
            fps=fps,
            ground=ground,
            height_m=height_m,
            joint_naming=joint_naming,
            keep_uvs=keep_uvs,
            ktx2_textures=ktx2_textures,
            output_format=output_format,
            output_formats=output_formats,
            output_packages=output_packages,
            pbr=pbr,
            rig=rig,
            skeleton=skeleton,
            source_model=source_model,
            symmetry=symmetry,
            target_polycount=target_polycount,
            texture=texture,
            texture_resolution=texture_resolution,
            topology=topology,
            usd_clips=usd_clips,
            usd_light_units=usd_light_units,
            usdz_root=usdz_root,
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
