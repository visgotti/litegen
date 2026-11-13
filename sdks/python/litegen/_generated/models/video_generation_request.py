from typing import TYPE_CHECKING, Any, Dict, List, Type, TypeVar, Union, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..models.video_camera import VideoCamera
from ..types import UNSET, Unset

if TYPE_CHECKING:
    from ..models.base_generation_request_metadata_type_0 import (
        BaseGenerationRequestMetadataType0,
    )
    from ..models.reference_image import ReferenceImage
    from ..models.source_ref import SourceRef
    from ..models.time_range import TimeRange


T = TypeVar("T", bound="VideoGenerationRequest")


@_attrs_define
class VideoGenerationRequest:
    """
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
        aspect_ratio (Union[None, Unset, str]):
        audio (Union[None, Unset, bool]): `false` for a silent clip, where the model can leave audio out
            (`supports_audio_toggle`).
        camera (Union[None, Unset, VideoCamera]):
        direction (Union[None, Unset, str]): An `extend`: add before (`backward`) or after (`forward`) the source.
        duration_seconds (Union[None, Unset, float]): Seconds of video, above 0, as the model's
            `params.duration_seconds`
            allows: within its range, and one of its `values` where it lists
            them; a whole-second model gets the nearest whole second. Absent means
            the model's own default. A model whose listing has no
            `duration_seconds` takes none: refused when `strict`, dropped otherwise.
        fps (Union[None, Unset, int]):
        loop (Union[None, Unset, bool]): `true` for a clip whose last frame is its first (`supports_loop`).
        replace (Union[None, Unset, str]): A `retake`'s track: `video`, `audio` or `both`.
        resolution (Union[None, Unset, str]):
        source_video (Union['SourceRef', None, Unset]):
        time_range (Union['TimeRange', None, Unset]):
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
    aspect_ratio: Union[None, Unset, str] = UNSET
    audio: Union[None, Unset, bool] = UNSET
    camera: Union[None, Unset, VideoCamera] = UNSET
    direction: Union[None, Unset, str] = UNSET
    duration_seconds: Union[None, Unset, float] = UNSET
    fps: Union[None, Unset, int] = UNSET
    loop: Union[None, Unset, bool] = UNSET
    replace: Union[None, Unset, str] = UNSET
    resolution: Union[None, Unset, str] = UNSET
    source_video: Union["SourceRef", None, Unset] = UNSET
    time_range: Union["TimeRange", None, Unset] = UNSET
    additional_properties: Dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> Dict[str, Any]:
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.source_ref import SourceRef
        from ..models.time_range import TimeRange

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

        aspect_ratio: Union[None, Unset, str]
        if isinstance(self.aspect_ratio, Unset):
            aspect_ratio = UNSET
        else:
            aspect_ratio = self.aspect_ratio

        audio: Union[None, Unset, bool]
        if isinstance(self.audio, Unset):
            audio = UNSET
        else:
            audio = self.audio

        camera: Union[None, Unset, str]
        if isinstance(self.camera, Unset):
            camera = UNSET
        elif isinstance(self.camera, VideoCamera):
            camera = self.camera.value
        else:
            camera = self.camera

        direction: Union[None, Unset, str]
        if isinstance(self.direction, Unset):
            direction = UNSET
        else:
            direction = self.direction

        duration_seconds: Union[None, Unset, float]
        if isinstance(self.duration_seconds, Unset):
            duration_seconds = UNSET
        else:
            duration_seconds = self.duration_seconds

        fps: Union[None, Unset, int]
        if isinstance(self.fps, Unset):
            fps = UNSET
        else:
            fps = self.fps

        loop: Union[None, Unset, bool]
        if isinstance(self.loop, Unset):
            loop = UNSET
        else:
            loop = self.loop

        replace: Union[None, Unset, str]
        if isinstance(self.replace, Unset):
            replace = UNSET
        else:
            replace = self.replace

        resolution: Union[None, Unset, str]
        if isinstance(self.resolution, Unset):
            resolution = UNSET
        else:
            resolution = self.resolution

        source_video: Union[Dict[str, Any], None, Unset]
        if isinstance(self.source_video, Unset):
            source_video = UNSET
        elif isinstance(self.source_video, SourceRef):
            source_video = self.source_video.to_dict()
        else:
            source_video = self.source_video

        time_range: Union[Dict[str, Any], None, Unset]
        if isinstance(self.time_range, Unset):
            time_range = UNSET
        elif isinstance(self.time_range, TimeRange):
            time_range = self.time_range.to_dict()
        else:
            time_range = self.time_range

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
        if aspect_ratio is not UNSET:
            field_dict["aspect_ratio"] = aspect_ratio
        if audio is not UNSET:
            field_dict["audio"] = audio
        if camera is not UNSET:
            field_dict["camera"] = camera
        if direction is not UNSET:
            field_dict["direction"] = direction
        if duration_seconds is not UNSET:
            field_dict["duration_seconds"] = duration_seconds
        if fps is not UNSET:
            field_dict["fps"] = fps
        if loop is not UNSET:
            field_dict["loop"] = loop
        if replace is not UNSET:
            field_dict["replace"] = replace
        if resolution is not UNSET:
            field_dict["resolution"] = resolution
        if source_video is not UNSET:
            field_dict["source_video"] = source_video
        if time_range is not UNSET:
            field_dict["time_range"] = time_range

        return field_dict

    @classmethod
    def from_dict(cls: Type[T], src_dict: Dict[str, Any]) -> T:
        from ..models.base_generation_request_metadata_type_0 import (
            BaseGenerationRequestMetadataType0,
        )
        from ..models.reference_image import ReferenceImage
        from ..models.source_ref import SourceRef
        from ..models.time_range import TimeRange

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

        def _parse_aspect_ratio(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        aspect_ratio = _parse_aspect_ratio(d.pop("aspect_ratio", UNSET))

        def _parse_audio(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        audio = _parse_audio(d.pop("audio", UNSET))

        def _parse_camera(data: object) -> Union[None, Unset, VideoCamera]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, str):
                    raise TypeError()
                camera_type_1 = VideoCamera(data)

                return camera_type_1
            except:  # noqa: E722
                pass
            return cast(Union[None, Unset, VideoCamera], data)

        camera = _parse_camera(d.pop("camera", UNSET))

        def _parse_direction(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        direction = _parse_direction(d.pop("direction", UNSET))

        def _parse_duration_seconds(data: object) -> Union[None, Unset, float]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, float], data)

        duration_seconds = _parse_duration_seconds(d.pop("duration_seconds", UNSET))

        def _parse_fps(data: object) -> Union[None, Unset, int]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, int], data)

        fps = _parse_fps(d.pop("fps", UNSET))

        def _parse_loop(data: object) -> Union[None, Unset, bool]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, bool], data)

        loop = _parse_loop(d.pop("loop", UNSET))

        def _parse_replace(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        replace = _parse_replace(d.pop("replace", UNSET))

        def _parse_resolution(data: object) -> Union[None, Unset, str]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(Union[None, Unset, str], data)

        resolution = _parse_resolution(d.pop("resolution", UNSET))

        def _parse_source_video(data: object) -> Union["SourceRef", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                source_video_type_1 = SourceRef.from_dict(data)

                return source_video_type_1
            except:  # noqa: E722
                pass
            return cast(Union["SourceRef", None, Unset], data)

        source_video = _parse_source_video(d.pop("source_video", UNSET))

        def _parse_time_range(data: object) -> Union["TimeRange", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                time_range_type_1 = TimeRange.from_dict(data)

                return time_range_type_1
            except:  # noqa: E722
                pass
            return cast(Union["TimeRange", None, Unset], data)

        time_range = _parse_time_range(d.pop("time_range", UNSET))

        video_generation_request = cls(
            model=model,
            extra=extra,
            metadata=metadata,
            n=n,
            negative_prompt=negative_prompt,
            prompt=prompt,
            reference_images=reference_images,
            seed=seed,
            strict=strict,
            aspect_ratio=aspect_ratio,
            audio=audio,
            camera=camera,
            direction=direction,
            duration_seconds=duration_seconds,
            fps=fps,
            loop=loop,
            replace=replace,
            resolution=resolution,
            source_video=source_video,
            time_range=time_range,
        )

        video_generation_request.additional_properties = d
        return video_generation_request

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
