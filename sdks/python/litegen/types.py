"""Static types for the 3D generation API and the shared generation rows.

Annotations only: ``TypedDict``\\ s and ``Literal`` aliases. The client returns
the plain ``dict`` that ``resp.json()`` produced, exactly as the image and video
namespaces always have, and nothing here parses, validates or converts anything
at runtime. A type checker (mypy, pyright) uses them to catch a misspelled key
or an impossible enum value before the request leaves the process.

Field names, required-ness and nullability mirror ``sdks/openapi.json``;
``tests/test_types_contract.py`` fails when the two drift apart. A key marked
``NotRequired`` may be absent from the response, which is different from being
``None``: the server omits ``assets`` entirely while a job is in flight, so read
it with ``.get("assets", [])``.

Why these and not the generated attrs classes in ``litegen._generated``: those
are objects with their own ``from_dict``/``to_dict``, while this client hands
back dicts — and a TypedDict *is* a dict to every caller at runtime.

No ``from __future__ import annotations`` here, deliberately. ``TypedDict``
computes ``__required_keys__`` from the annotations when the class is created,
and string annotations hide ``Required[...]``/``NotRequired[...]`` from that
computation (the ``typing`` docs say as much under ``__required_keys__``). The
drift test reads those attributes, so they have to be right.
"""
from typing import Any

from typing_extensions import Literal, NotRequired, Required, TypedDict

# ─── Vocabularies ────────────────────────────────────────────────────────────

Model3dFormat = Literal["glb", "gltf", "obj", "stl", "ply", "fbx", "usdz", "usda", "3mf", "dae"]
"""A mesh container: the whole vocabulary any 3D model may deliver. ``glb`` is
the one every 3D model can emit. Which of the others a given model can deliver
is the server's decision, advertised per model by ``GET /v1/models/{id}``:
``params.output_formats.enum_values``, with ``derivable_values`` naming those
litegen converts locally (any subset of which costs one provider job)."""

Model3dTopology = Literal["triangle", "quad"]
"""Mesh face type, where the model lets you choose."""

Model3dSymmetry = Literal["off", "auto", "on"]
"""Whether the model should enforce a symmetric mesh."""

Model3dSkeleton = Literal[
    "humanoid", "quadruped", "hexapod", "octopod", "avian", "serpentine", "aquatic", "other"
]
"""The skeleton a rig is built for. Which ones a model rigs is its
``params.skeleton.enum_values``."""

Model3dJointNaming = Literal["vendor", "vrm", "mixamo", "unity", "unreal"]
"""The joint names a rig is delivered with; ``vendor`` keeps the vendor's own."""

Model3dAnimationPreset = Literal[
    "idle", "walk", "run", "jump", "fall", "attack_melee", "attack_ranged",
    "cast", "hurt", "death", "crouch", "climb", "turn",
]
"""The canonical animation presets. A model maps the ones it plays onto its own
preset ids (``params.animations.presets``); a clip may name either."""

Model3dAssetKind = Literal["mesh", "texture", "preview", "original"]
"""What a :class:`Model3dAsset` is: the mesh itself, a texture map, a preview
render, or ``original`` — the vendor's own file, kept beside a mesh litegen
changed to deliver it in glTF's conventions."""

GenerationStatus = Literal["pending", "processing", "completed", "failed", "cancelled"]
"""Lifecycle of an async job. ``completed``, ``failed`` and ``cancelled`` are
terminal: the job never changes again, and polling stops at the first of them."""

RefImageKind = Literal["base64", "url", "blob", "generation"]
"""How :class:`ReferenceImage` ``value`` is encoded. ``generation`` is a stored
litegen image generation's id, read back by the server."""

SourceKind = Literal["generation", "url", "base64"]
"""How :class:`SourceRef` ``value`` is to be read: a litegen generation id, a
public URL, or base64 bytes."""

CostSource = Literal["dynamic", "estimated"]
"""``dynamic``: the vendor reported the cost. ``estimated``: from LiteGen's pricing table."""

MediaType = Literal["image", "video", "model3d"]
"""The ``media_type`` filter accepted by ``client.models.list``."""

ModelTask = Literal[
    "generate", "rig", "animate", "texture", "remesh", "segment",
    "multiview", "background_swap", "extend", "retake", "video_edit",
]
"""What a catalog model does (``capabilities.task``, and ``task`` on its
schema): ``generate``, or an operation on an existing asset. Operations are
submitted to their modality's usual generation endpoint."""


# ─── Requests ────────────────────────────────────────────────────────────────


ImageView = Literal[
    "front", "front-left", "left", "back-left", "back", "back-right", "right", "front-right", "top", "bottom",
]
"""A view of a subject, named for the side the camera sees. Its yaw
(``yaw_deg`` on a multiview image) is the camera's position around +Y from the
subject's front (+Z) towards +X: front 0, front-left 45, left 90, back-left 135,
back 180, back-right 225, right 270, front-right 315; top and bottom have none.
A multiview image model takes ``views=`` from this list; a multiview-to-3D model
takes ``view-<name>`` references."""

VideoCamera = Literal["fixed", "auto"]
"""A video's camera (``camera=`` on ``videos.generate``): ``fixed`` holds the shot
still, on models with ``supports_fixed_camera``."""

ImageOutputFormat = Literal["png", "webp", "jpeg"]
"""An image container (``output_format=`` on ``images.generate``): one of the
model's ``capabilities.output_formats``; a ``lossy_only`` model offers no PNG."""

ImageBackground = Literal["transparent", "opaque"]
"""An image's background (``background=``): ``transparent`` is a real alpha
channel, on models with ``supports_transparent_background``; it needs PNG or WebP."""

ModelStatus = Literal["active", "deprecated", "retired"]
"""Where a model is in its vendor's lifecycle (``status`` on ``GET /v1/models``):
``deprecated`` models are still served until the day after their
``retires_at``; ``replaced_by`` names the catalog model to move to."""

KeyframeTiming = Literal["any", "even"]
"""Where a video model's ``keyframe`` references may sit (``capabilities.keyframe_timing``):
``any`` time each, or ``even`` spacing from the start of the clip to its end."""

MaskConvention = Literal["white_edits", "black_edits", "transparent_edits"]
"""Which pixels of a mask edit. Callers always send ``white_edits`` (white,
128 or more, edits); a model's catalog row names its vendor's own, and
litegen converts."""

RegionInput = Literal["box", "mask"]
"""How a model receives a ``region``: its own box, or drawn as its mask."""

PreservesUnmasked = Literal["exact", "similar", "unspecified"]
"""What a vendor promises about pixels outside an edit, in its own words."""


class PriceRule(TypedDict):
    """One rule of a model's ``pricing.variable_pricing`` (from ``GET /v1/models``).

    Rules apply in order to a running price that starts at ``base_cost_usd``.
    When every ``when`` entry matches the request — a param as sent or its
    default, an ``extra.<field>``, or a derived fact (``views``,
    ``keyframes``, ``animations``, ``duration_seconds``, ``megapixels``), with
    ``"*"`` meaning any value present — the rule's one effect applies: a price
    (``usd``), a rate (``per_second_usd``, ``per_megapixel_usd``,
    ``per_item_usd`` × the ``per_item`` count) or an adjustment
    (``multiply``, ``add_usd``). ``/cost``, the quota reservation and the
    charge all use them.
    """

    when: NotRequired[dict[str, Any]]
    usd: NotRequired[float | None]
    per_second_usd: NotRequired[float | None]
    per_megapixel_usd: NotRequired[float | None]
    per_item_usd: NotRequired[float | None]
    per_item: NotRequired[str | None]
    multiply: NotRequired[float | None]
    add_usd: NotRequired[float | None]


class Region(TypedDict):
    """A rectangle of the init image, in its pixels: top-left origin, ends
    exclusive (``x..x+w`` by ``y..y+h``). Pass it to ``images.generate`` as
    ``region=`` to edit only that part, with ``mask_grow_px=`` to grow it."""

    x: int
    y: int
    w: int
    h: int


SegmentationPromptType = Literal["point", "box", "text"]
"""A prompt type a model segments by: its ``capabilities.segmentation_prompts``."""


class SegmentationPoint(TypedDict):
    """A spot on the object (``label`` 1, the default) or one to leave out of
    it (``label`` 0), in the image's pixels."""

    type: Literal["point"]
    x: int
    y: int
    label: NotRequired[int]


class SegmentationBox(TypedDict):
    """A box around the object: pixels, top-left origin, ends exclusive."""

    type: Literal["box"]
    x: int
    y: int
    w: int
    h: int


class SegmentationText(TypedDict):
    """What to select, named: every instance of it. Sent alone."""

    type: Literal["text"]
    text: str


SegmentationPrompt = SegmentationPoint | SegmentationBox | SegmentationText
"""One entry of ``images.segment(prompts=…)``. Points and boxes together select
one object; a text prompt selects every instance of what it names."""


class SegmentationMask(TypedDict):
    """One selected object from ``images.segment``."""

    png_base64: str
    """A grayscale PNG the image's size: white (255) where selected, black elsewhere."""
    score: float | None
    """The vendor's confidence, 0–1; ``None`` when it gives none."""
    box: Region
    """The smallest box around the white pixels, measured by LiteGen."""


SegmentOrigin = Literal["source", "generated"]
"""Where a stretch of a video operation's clip came from."""

ExtendOutput = Literal["full", "added"]
"""What an ``extend`` model returns: the whole clip, or only the added part."""


class TimeRange(TypedDict):
    """A window of a clip, in seconds from its start (``retake``, ``video_edit``)."""

    start_s: float
    end_s: float


class VideoSegment(TypedDict):
    """One stretch of a finished video operation's clip, in its own seconds."""

    start_s: float
    end_s: float
    origin: SegmentOrigin


LayoutKind = Literal["object", "text"]
"""What a layout box holds: an object (its ``prompt``) or words (its ``text``)."""


class LayoutBox(TypedDict):
    """Where a layout box sits, as fractions of the canvas (0 to 1)."""

    x: float
    y: float
    w: float
    h: float


class LayoutItem(TypedDict):
    """One box of ``images.generate(layout=[...])``, where the model's listing
    says ``supports_layout``. Boxes guide the layout; no vendor clips to them."""

    box: LayoutBox
    kind: LayoutKind
    prompt: NotRequired[str | None]
    text: NotRequired[str | None]


class Expand(TypedDict):
    """Pixels to add to each side of the init image's canvas (``expand=`` on
    ``images.generate``, where the model's listing says ``supports_expand``).
    The result is the whole canvas, with the source at (``left``, ``top``)."""

    left: NotRequired[int]
    top: NotRequired[int]
    right: NotRequired[int]
    bottom: NotRequired[int]


class ReferenceImage(TypedDict):
    """An input image. On a 3D model its ``role`` selects the mode; there is no
    separate mode switch:

    * ``"init"``: image-to-3D from a single picture.
    * ``"view-<name>"`` for an :data:`ImageView` (``"view-front"``,
      ``"view-left"``, …): multiview-to-3D, one image per view. The front is
      always needed, and an ``init`` image cannot ride along. A model whose
      ``ref_inputs.view_order`` is set reads its views by position, so a view
      needs every view before it in that order.

    Which roles a model takes is in its ``ref_inputs`` (``GET /v1/models/{id}``).
    Leaving ``role`` out uses the model's default role.
    """

    type: RefImageKind
    value: str
    """The image itself, encoded as ``type`` says: base64 data, a URL, or a blob reference."""
    role: NotRequired[str | None]
    time_s: NotRequired[float | None]
    """Seconds from the start of the clip, on a video ``keyframe`` reference:
    the video shows this image at that time."""


class SourceRef(TypedDict):
    """The mesh an operation (``rig``, ``animate``, …) works on.

    ``{"type": "generation", "value": "litegen-3d-…"}`` chains a step onto an
    earlier one; ``url`` and ``base64`` bring your own mesh and must name its
    ``format``. The server reads the source, refuses one the model's vendor
    would refuse before anything is billed (``source_too_many_faces``,
    ``source_not_rigged``, …), and converts it to a container the vendor takes.
    """

    type: SourceKind
    value: str
    format: NotRequired[Model3dFormat | None]


class AnimationRequest(TypedDict):
    """One clip an ``animate`` model makes: exactly one of ``preset`` and
    ``prompt``. Delivered as a glTF animation named exactly ``name``, in
    request order."""

    name: str
    """Unique within the request, 1–64 characters of ``[A-Za-z0-9_.-]``."""
    preset: NotRequired[str | None]
    """A :data:`Model3dAnimationPreset` or one of the model's own preset ids."""
    prompt: NotRequired[str | None]
    """Text-to-motion, where the model offers it."""
    duration_s: NotRequired[float | None]
    """A prompt clip's length, within the model's ``text_to_motion`` range."""
    in_place: NotRequired[bool | None]
    """Keep the root in place horizontally; the manifest says who did."""
    loop: NotRequired[bool | None]
    """A loop hint, recorded in the manifest."""


class Model3dGenerateParams(TypedDict, total=False):
    """Body of ``POST /v1/models3d/generations``, and of ``POST /v1/models3d/cost``,
    which prices the same request without starting a job.

    Only ``model`` is required. Leave a field out (or pass ``None``, which the
    SDK strips before sending) to get the model's own default.

    Each model supports its own subset of these fields, listed by
    ``GET /v1/models/{id}``. With ``strict`` on (the default), sending one it
    does not support is a 400. With ``strict=False`` (lax mode) the server drops
    it and carries on, and the job handle's ``dropped_params`` names what went.

    ``n`` is deliberately absent. It exists on the request base shared with
    images and video, but a 3D job always produces exactly one result and the
    3D validator never reads it, so offering it would only suggest otherwise.
    """

    model: Required[str]
    """Model id, e.g. ``"fal/hyper3d-rodin"``."""
    prompt: str
    """What to build. Optional, because image-only models (``fal/trellis``,
    ``fal/hunyuan3d-v2``) take no prompt: they build from an ``init`` reference
    image. A model that needs a prompt rejects a request without one with a 400
    (``prompt_required``)."""
    negative_prompt: str | None
    seed: int | None
    reference_images: list[ReferenceImage]
    """Input images. Their ``role`` picks image-to-3D or multiview-to-3D; see
    :class:`ReferenceImage`."""
    strict: bool
    """``True`` by default: an unsupported param is a 400. ``False`` drops it
    instead and reports it through the job's ``dropped_params``."""
    extra: dict[str, Any]
    """Vendor-specific fields passed through to the provider. Only keys on the
    model's ``extra_allowlist`` are accepted."""
    metadata: Any
    """Arbitrary JSON, accepted on every generation request."""
    output_formats: list[Model3dFormat] | None
    """Mesh containers to deliver, e.g. ``["glb", "fbx"]``. This is a promise,
    not a hint: a ``completed`` generation carries a ``mesh`` asset in every
    format listed, or the generation fails. Defaults to the model's declared
    default, and to ``["glb"]`` when it declares none."""
    output_format: Model3dFormat | None
    """Deprecated single-format form of ``output_formats``. The server folds it
    into ``output_formats``; prefer the list."""
    texture: bool | None
    """Generate textures rather than bare geometry."""
    pbr: bool | None
    """PBR materials, where the model supports them."""
    rig: bool | None
    """Auto-rig: emit a skeleton, where the model supports it."""
    symmetry: Model3dSymmetry | None
    topology: Model3dTopology | None
    target_polycount: int | None
    """Approximate polygon budget for the mesh."""
    source_model: SourceRef | None
    """The mesh an operation works on; required by every model whose task is
    not ``generate``. See :class:`SourceRef`."""
    skeleton: Model3dSkeleton | None
    """The skeleton to rig for (rig models)."""
    joint_naming: Model3dJointNaming | None
    """The joint names to deliver the rig with (rig models)."""
    height_m: float | None
    """The delivered mesh's height in metres: LiteGen scales any mesh to it,
    and a rig model that takes a height gets it too."""
    ground: bool | None
    """Put the mesh's lowest point at y = 0 (applied by LiteGen on delivery)."""
    center: bool | None
    """Centre the mesh's bounds on x and z (applied by LiteGen on delivery)."""
    animations: list[AnimationRequest] | None
    """The clips an ``animate`` model makes, in order."""
    fps: int | None
    """Frames per second of the delivered clips."""
    texture_resolution: int | None
    """A ``texture`` model's map size in pixels, one of its
    ``texture_resolutions``."""
    keep_uvs: bool | None
    """Paint the source's own UV layout rather than a new one (texture models)."""


# ─── Responses ───────────────────────────────────────────────────────────────


class UsageInfo(TypedDict):
    """What a finished generation cost."""

    cost_usd: float
    """Provider cost, in US dollars."""
    tokens: int
    """The same cost in LiteGen's internal token units."""
    cost_source: CostSource


class ImageSegmentationResponse(TypedDict):
    """What ``images.segment`` returns (``POST /v1/images/segmentations``)."""

    id: str
    created: int
    model: str
    provider: str
    masks: list[SegmentationMask]
    """One per object selected, best score first; empty when nothing was found."""
    usage: NotRequired[UsageInfo | None]


class CostEstimate(TypedDict):
    """Returned by ``estimate_cost``: the price of a request, before running it."""

    base_cost_usd: float
    """The provider's cost, in US dollars."""
    markup_usd: float
    """Markup on top of the base cost, in US dollars."""
    total_cost_usd: float
    """``base_cost_usd`` plus ``markup_usd``."""
    tokens_required: int
    """The total in LiteGen's internal token units."""
    cost_source: CostSource
    breakdown: NotRequired[Any]
    """Free-form pricing details; the shape varies by model."""


class SceneBounds(TypedDict):
    """World-space bounds of a delivered mesh, in metres: ``[x, y, z]`` each."""

    min: list[float]
    max: list[float]


class ScenePart(TypedDict):
    """One part of a segmented mesh: a glTF node, named with the vendor's label
    or ``part_<n>``, measured by LiteGen."""

    name: str
    node: int
    """The part's index in the delivered glTF's ``nodes``."""
    triangle_count: int
    bounds: NotRequired[SceneBounds | None]


class SceneMaterial(TypedDict):
    name: NotRequired[str | None]
    maps: list[str]
    """``base_color``, ``metallic_roughness``, ``normal``, ``occlusion``, ``emissive``."""


class SceneSkin(TypedDict):
    """A delivered mesh's skeleton, as litegen read it."""

    skeleton: NotRequired[str | None]
    """``humanoid`` when a known naming scheme matched, else what the request asked for."""
    joint_count: int
    root: NotRequired[str | None]
    bind_pose: str
    """``t``, ``a`` or ``unknown``, from the upper-arm angle at rest."""
    joints: list[str]
    """Joint names, unique, in skin order."""
    humanoid: NotRequired[dict[str, str] | None]
    """VRM 1.0 humanoid bone → joint name, for a rig litegen recognises."""


class SceneAnimation(TypedDict):
    name: str
    duration_s: float
    in_place: bool
    """Measured: the root ends within 1 cm of where it started, horizontally."""
    in_place_by: NotRequired[str | None]
    """``vendor`` (arrived in place) or ``litegen`` (litegen removed the root's travel)."""
    root_travel_m: list[float]
    loop_requested: NotRequired[bool | None]


class SceneManifest(TypedDict):
    """What litegen measured in a delivered mesh, read from the file's own bytes.

    Every delivered mesh is in glTF's conventions — metres, ``+y`` up, facing
    ``+z`` — and ``oriented_by`` says whether the vendor or litegen made it so.
    ``front`` is ``"unverified"`` for a model whose orientation nobody has
    proven yet; litegen never reports a guess.
    """

    units: str
    up: str
    front: str
    oriented_by: str
    bounds: NotRequired[SceneBounds | None]
    vertex_count: int
    triangle_count: int
    uv_sets: int
    vertex_colors: bool
    materials: list[SceneMaterial]
    skin: NotRequired[SceneSkin | None]
    animations: list[SceneAnimation]
    extensions_used: list[str]
    """glTF extensions the file uses. None is ever required."""
    parts: NotRequired[list[ScenePart] | None]
    """A ``segment`` model's parts; absent on every other model's mesh."""


class Model3dAsset(TypedDict):
    """One file produced by a 3D generation. ``url`` is always absolute."""

    kind: Model3dAssetKind
    url: str
    format: str
    """File format: a :data:`Model3dFormat` for a mesh, an image format such as
    ``"png"`` or ``"jpg"`` for a texture or preview. Typed ``str`` because it
    covers both."""
    size_bytes: NotRequired[int | None]
    polycount: NotRequired[int | None]
    """Meshes only."""
    width: NotRequired[int | None]
    """Textures and previews only, in pixels."""
    height: NotRequired[int | None]
    """Textures and previews only, in pixels."""
    sha256: NotRequired[str | None]
    """Lowercase hex SHA-256 of the stored file."""
    scene: NotRequired[SceneManifest | None]
    """Meshes only: what litegen's own reader found in this very file."""


class Model3dGenerationResponse(TypedDict):
    """A 3D job: what ``POST /v1/models3d/generations`` returns on submit, and
    what each ``GET /v1/models3d/{id}`` poll returns after it.

    A ``completed`` job carries a ``kind == "mesh"`` asset for every requested
    container (so always at least one); when one of them is glb, that is the
    primary mesh the generation row's ``result_url`` points at. A ``failed`` job
    explains itself in ``error``.
    """

    id: str
    status: GenerationStatus
    model: str
    provider: str
    progress: int
    """Unified 0-100 progress. Providers that report nothing finer step toward 100."""
    created: int
    """Unix timestamp, in seconds."""
    assets: NotRequired[list[Model3dAsset]]
    """Present once the job is ``completed``; absent while it is in flight."""
    error: NotRequired[str | None]
    """Why the job failed."""
    usage: NotRequired[UsageInfo | None]


class GenerationAsset(TypedDict):
    """One file litegen kept for a generation (``GET /v1/generations/{id}/assets``).
    ``index`` is what :meth:`download_asset` takes."""

    index: int
    kind: str
    format: str
    url: str
    size_bytes: NotRequired[int | None]
    sha256: NotRequired[str | None]
    width: NotRequired[int | None]
    height: NotRequired[int | None]
    duration_s: NotRequired[float | None]
    fps: NotRequired[float | None]
    has_audio: NotRequired[bool | None]
    """Videos: whether the file carries an audio track (read from its bytes)."""


class GenerationAssetList(TypedDict):
    """``GET /v1/generations/{id}/assets``: every file kept, in order."""

    generation_id: str
    data: list[GenerationAsset]


class Generation(TypedDict):
    """A persisted generation row (image, video or 3D), as served by
    ``/v1/generations``. Timestamps are ISO 8601 strings."""

    id: str
    model: str
    provider: str
    media_type: str
    """``"image"``, ``"video"`` or ``"model3d"``."""
    status: GenerationStatus
    progress: int
    """Unified 0-100 progress."""
    cost_usd: float
    created_at: str
    completed_at: NotRequired[str | None]
    key_id: NotRequired[str | None]
    """The API key that submitted the job; ``None`` for the master key."""
    provider_job_id: NotRequired[str | None]
    result_url: NotRequired[str | None]
    """The primary result once completed. For 3D this is the glb mesh when there is one."""
    error_message: NotRequired[str | None]
    metadata: NotRequired[Any]
    org_id: NotRequired[str | None]
    app_id: NotRequired[str | None]


class GenerationPage(TypedDict):
    """One page of ``GET /v1/generations``."""

    data: list[Generation]
    total: int
    """Rows across all pages."""
    page: int
    per_page: int
    total_pages: int


# ─── Mesh conversion (``/v1/utils/models3d/*``) ───────────────────────────────


class Model3dConvertFormats(TypedDict):
    """What ``models3d.convert_formats()`` returns: what the converter accepts."""

    formats: list[Model3dFormat]
    """Containers usable as both ``from_format`` and ``to_format`` — every pair
    converts, in both directions. Narrower than :data:`Model3dFormat`, and it
    grows with litegen's converter, so read it rather than hard-coding it."""
    max_input_bytes: int
    """The largest file accepted, in bytes; a bigger one is a 413."""


class Model3dConversion(TypedDict):
    """What ``models3d.convert()`` returns. Built by the SDK from the response's
    bytes and headers — not a wire schema."""

    content: bytes
    """The converted file."""
    content_type: str
    """Its media type, as the server sent it (e.g. ``"model/obj"``)."""
    filename: str
    """A file name for it: the server's Content-Disposition (``model.<to>``)."""
    format: Model3dFormat
    """The container it is in — the ``to_format`` that was asked for."""
    losses: list[str]
    """What this file lost on the way to the target container (e.g.
    ``["materials", "uvs"]``), from the ``X-Litegen-Conversion-Losses`` header.
    Worked out from the input's content, not the format pair; ``[]`` when
    nothing was lost. Known kinds: animation, skins, morph_targets, normals,
    uvs, vertex_colors, textures, image_formats, materials, alpha, hierarchy,
    meshes, texture_sampling, texture_transforms, geometry — a kind added later
    passes through as-is."""
