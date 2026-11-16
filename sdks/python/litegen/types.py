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

Model3dFormat = Literal["glb", "gltf", "obj", "stl", "ply", "fbx", "usdz", "usda", "usdc", "3mf", "dae"]
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
Model3dFbxColorSpace = Literal["srgb", "linear"]
Model3dFormatSource = Literal["native", "glb"]
Model3dKtx2Textures = Literal["png", "keep"]
"""What delivery does with a KTX2 (Basis Universal) texture: ``"png"`` (the
default) or ``"keep"``. See ``Model3dGenerateParams.ktx2_textures``."""
Model3dLightUnits = Literal["photometric", "blender"]
"""How a delivered or read FBX or USD stores its light strengths:
``"photometric"`` (the default; the scene's own, glTF's candela and lux) or
``"blender"`` (Blender's ``energy``, so the lights open in Blender as bright
as the glTF does). See ``Model3dGenerateParams.fbx_light_units``."""
Model3dDaeMorphs = Literal["bake", "keep"]
"""How a Collada (``dae``) file LiteGen writes stores a mesh's morph targets:
``"bake"`` (the default; each node's mesh at that node's weights, which every
Collada reader opens) or ``"keep"`` (``<morph>`` controllers with their weight
animation, which SceneKit and Godot read and three.js and assimp refuse). See
``Model3dGenerateParams.dae_morphs``."""
Model3dUsdzRoot = Literal["usda", "usdc"]
"""What the root layer of a ``usdz`` LiteGen writes is: ``"usda"`` (the
default; text, which ``usdchecker --arkit`` accepts and ``<model-viewer>``
hands to AR Quick Look) or ``"usdc"`` (the binary crate, as Apple's own
packagers write it). See ``Model3dGenerateParams.usdz_root``."""
Model3dUsdClips = Literal["all", "first"]
"""Which animation clips a USD file LiteGen writes holds: ``"all"`` (every
clip as an alternative, chosen by a variant set ``Animation``; rigid node
motion is the first clip's only, the others' named in the losses) or
``"first"`` (the first, the rest named in the losses). See
``Model3dGenerateParams.usd_clips``."""

Model3dAnimationPreset = Literal[
    "idle", "walk", "run", "jump", "fall", "attack_melee", "attack_ranged",
    "cast", "hurt", "death", "crouch", "climb", "turn",
]
"""The canonical animation presets. A model maps the ones it plays onto its own
preset ids (``params.animations.presets``); a clip may name either."""

Model3dAssetKind = Literal["mesh", "texture", "preview", "original", "package"]
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
"""Where a model is in its vendor's lifecycle (``status`` on ``GET /v1/models``
and ``GET /v1/models/{id}``, derived from its dates and ignored when a schema
is posted back): ``deprecated`` models are still served until the day after their
``retires_at``; ``replaced_by`` names the catalog model to move to."""

KeyframeTiming = Literal["any", "even"]
"""Where a video model's ``keyframe`` references may sit (``capabilities.keyframe_timing``):
``any`` time each, or ``even`` spacing from the start of the clip to its end."""

FramesSetShape = Literal["always", "without_references"]
"""When a video model's frame image sets the clip's shape
(``params.aspect_ratio.frames_set_shape`` in ``GET /v1/models/{id}``): with a
``first_frame``, ``last_frame`` or ``init`` image its vendor takes no ratio, so
``aspect_ratio`` is refused (``param_unsupported``) when ``strict`` and dropped
and named otherwise. ``always``: with any frame image; ``without_references``:
unless a ``reference`` image goes too, which sends the request to a vendor
mode that takes a ratio. Absent: the ratio applies with any image."""

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
    (``multiply``, ``add_usd``, or ``multiply_per_megapixel_above``: the
    price × megapixels ÷ that anchor, for an image larger than it).
    ``/cost``, the quota reservation and the charge all use them.
    """

    when: NotRequired[dict[str, Any]]
    usd: NotRequired[float | None]
    per_second_usd: NotRequired[float | None]
    per_megapixel_usd: NotRequired[float | None]
    per_item_usd: NotRequired[float | None]
    per_item: NotRequired[str | None]
    multiply: NotRequired[float | None]
    multiply_per_megapixel_above: NotRequired[float | None]
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


Direction = Literal["south", "south-east", "east", "north-east", "north", "north-west", "west", "south-west"]
"""A facing of a ``directions`` request, clockwise from towards the viewer."""


class PixelSize(TypedDict):
    """A native pixel-art model's output edge, in pixels."""

    min: int
    max: int


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
    ``prompt``. Delivered as a glTF animation, in request order, named its
    ``name``, or the name litegen gives a clip that leaves it out."""

    name: NotRequired[str | None]
    """The clip's name in the delivered file: unique within the request, 1–64
    characters of ``[A-Za-z0-9_.-]``. Leave it out (or send ``None``) and
    litegen names the clip: a preset clip after its canonical preset
    (``walk``), else after its preset id with every other character as ``_``;
    a prompt clip, or one whose preset leaves no name, ``clip-<n>``, its place
    in the list; ``-2``, ``-3``, … on a name already taken. A name you send is
    kept as sent; an empty one is refused."""
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
    does not support, or a field the request has no such name for, is a 400.
    With ``strict=False`` (lax mode) the server drops it and carries on, and
    the job handle's ``dropped_params`` names what went.

    ``n`` is deliberately absent. It exists on the request base shared with
    images and video, but a 3D job always makes exactly one mesh: the server
    refuses ``n`` above 1 (``param_out_of_range``), and 0 or 1 is what leaving
    it out already means, so offering it would only suggest otherwise.
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
    """``True`` by default: a param the model does not take, or a field the
    request has no such name for (one inside an object by its path, such as
    ``animations[0].duration``), is a 400 ``param_unsupported``. ``False``
    drops it instead and reports it through the job's ``dropped_params``. A
    bad value for a param the model does take is refused either way."""
    extra: dict[str, Any]
    """Vendor-specific fields passed through to the provider: only keys on the
    model's ``extra_allowlist``, each of the type its ``extra_params`` entry
    gives when it has one (a wrong value is a 400 in either mode, with
    ``param`` ``"extra.<key>"``). Strict mode refuses any other key
    (``extra_key_unsupported``); ``strict=False`` drops it and names it in
    ``dropped_params``."""
    metadata: dict[str, str]
    """Your own labels for the job (string values, at most 1 KB as JSON). Kept
    on the generation, echoed on every answer and webhook, and filters
    ``logs.list`` and ``stats.get``."""
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
    and a rig model that takes a height gets it too. Above 0; the model's
    ``litegen_params`` (``GET /v1/models/{id}``) give the ceiling, or its
    ``params`` for a rig model that declares its own ``height_m``."""
    ground: bool | None
    """Put the mesh's lowest point at y = 0 (applied by LiteGen on delivery;
    listed in every 3D model's ``litegen_params``)."""
    center: bool | None
    """Centre the mesh's bounds on x and z (applied by LiteGen on delivery;
    listed in every 3D model's ``litegen_params``)."""
    output_packages: bool | None
    """Also deliver a zip per requested container whose mesh has files to go
    beside it (an OBJ's ``.mtl`` and textures): ``"package"`` assets. Refused
    (``option_needs_keeping``) when nothing is kept."""
    animations: list[AnimationRequest] | None
    """The clips an ``animate`` model makes, in order: one glTF animation per
    entry, named its ``name``, or the name litegen gives a clip that leaves it
    out."""
    fps: int | None
    """Frames per second of the delivered clips."""
    fbx_color_space: Model3dFbxColorSpace | None
    """How FBX material and light colours are stored, when LiteGen reads the
    vendor's FBX or writes one for this request (a vendor FBX delivered as it
    is keeps the vendor's): ``"srgb"`` (the default; three.js reads them so)
    or ``"linear"`` (Blender, ufbx and assimp read both so, and Unity and
    Unreal material colours). Vertex colours are sRGB either way."""
    fbx_light_units: Model3dLightUnits | None
    """The units FBX light strengths are stored in when LiteGen reads the
    vendor's FBX or writes one for this request (a vendor FBX delivered as it
    is keeps the vendor's units): ``"photometric"`` (the default; the scene's
    own, as three.js's FBXLoader and ufbx read them: ``Intensity`` is 100 × the
    candela or lux) or ``"blender"`` (100 × the ``energy`` Blender's own glTF
    importer gives the same light, so the lights open in Blender as bright as
    the glTF does, and a vendor FBX that Blender wrote reads as that light).
    Colours are in ``fbx_color_space`` either way."""
    usd_light_units: Model3dLightUnits | None
    """The units USD light strengths are stored in when LiteGen reads the
    vendor's ``usdz``, ``usda`` or ``usdc`` or writes one for this request (a vendor USD
    delivered as it is keeps the vendor's units): ``"photometric"`` (the
    default; UsdLux's own, by the schema) or ``"blender"`` (what
    Blender's USD exporter writes for the same light, so the lights open in
    Blender as bright as the glTF does). A file LiteGen wrote keeps its exact
    strengths in ``customData``, which wins on read either way."""
    dae_morphs: Model3dDaeMorphs | None
    """How the Collada (``dae``) LiteGen writes stores a mesh's morph targets,
    when LiteGen writes one for this request (a vendor Collada delivered as it
    is keeps the vendor's): ``"bake"`` (the default) applies the targets at
    each node's weights and drops their weight animation, which every Collada
    reader opens; ``"keep"`` writes them as ``<morph>`` controllers with their
    weight animation, which SceneKit (and so Xcode) and Godot read, while
    three.js's ColladaLoader and assimp refuse the whole file. Refused
    (``option_needs_keeping``) when nothing is kept."""
    usdz_root: Model3dUsdzRoot | None
    """What the root layer of a ``usdz`` LiteGen writes for this request is (a
    vendor USDZ delivered as it is keeps the vendor's): ``"usda"`` (the
    default) is text, which ``usdchecker --arkit`` accepts and which
    ``<model-viewer>`` hands to AR Quick Look; ``"usdc"`` is the binary crate,
    as Apple's own packagers write it. Either way the root is the package's
    first entry, 64-byte aligned. ``"usdc"`` is refused
    (``option_needs_keeping``) when nothing is kept."""
    usd_clips: Model3dUsdClips | None
    """Which animation clips the ``usda``, ``usdc`` or ``usdz`` LiteGen writes
    for this request holds (a vendor USD delivered as it is keeps the
    vendor's): ``"all"`` keeps every clip as an alternative (a SkelAnimation
    prim for each, chosen by a variant set ``Animation``, as Apple's own
    tooling lays them out), ``"first"`` keeps the first and names the rest in
    the losses. Under ``"all"`` every clip's skeletal and blend-shape animation
    is kept; rigid (node) motion is the first clip's only, the other clips' is
    named in the losses, and a scene whose clips are all rigid writes as
    ``"first"``. Unset is the container's default: all for ``usda`` and
    ``usdc``, the first for ``usdz`` until AR Quick Look's behaviour with
    variants is checked on a device. ``"all"`` is refused
    (``option_needs_keeping``) when nothing is kept."""
    ktx2_textures: Model3dKtx2Textures | None
    """What delivery does with a KTX2 (Basis Universal) texture, such as the
    images of a GLB that requires ``KHR_texture_basisu``. ``"png"`` (the
    default) re-encodes it as PNG in every container, so no delivered file
    needs an extension to read, and the vendor's own file is delivered beside
    it as ``original``. ``"keep"`` leaves it as the vendor made it in the
    ``glb`` and ``gltf`` containers, which then require ``KHR_texture_basisu``
    (for viewers that upload KTX2 to the GPU); every other container still
    gets PNG. Refused (``option_needs_keeping``) when nothing is kept."""
    format_source: Model3dFormatSource | None
    """Where a promised ``fbx`` or ``usdz`` comes from: ``"native"`` (the
    default) asks the vendor for its own export; ``"glb"`` asks for the
    vendor's GLB and LiteGen makes the rest from it. One vendor job either way."""
    texture_resolution: int | None
    """A ``texture`` model's map size in pixels, one of its
    ``texture_resolutions``."""
    keep_uvs: bool | None
    """Paint the source's own UV layout rather than a new one (texture models)."""


# ─── Responses ───────────────────────────────────────────────────────────────


class RateLimit(TypedDict):
    """``X-RateLimit-*``, sent for a key with a requests-per-minute limit."""

    limit: int
    remaining: int
    reset_at: int | None
    """Unix seconds when the window resets."""


class ResponseMeta(TypedDict):
    """What :func:`litegen.response_meta` reads off a response's headers."""

    dropped_params: list[str]
    """``X-Litegen-Dropped-Params``: what ``strict: false`` dropped instead of
    refusing, by name, and a last ``+N more`` when there were more than the
    header lists."""
    rate_limit: RateLimit | None
    idempotent_replayed: bool
    """``Idempotent-Replayed: true``: the stored first answer to this ``Idempotency-Key``, billed nothing."""
    retry_after: float | None
    """``Retry-After``, in seconds (a 429 or 503)."""


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
    metadata: NotRequired[dict[str, str] | None]
    """The request's ``metadata``, as sent."""


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
    notes: NotRequired[list[str]]
    """What delivery could not keep or do for this file: a texture it could
    not decode, a joint naming the rig could not take, what this container
    cannot hold. Absent when nothing was lost."""


class Model3dAsset(TypedDict):
    """One file produced by a 3D generation. ``url`` is always absolute."""

    kind: Model3dAssetKind
    url: str
    format: str
    """File format: a :data:`Model3dFormat` for a mesh, an image format such as
    ``"png"`` or ``"jpg"`` for a texture or preview. Typed ``str`` because it
    covers both."""
    size_bytes: NotRequired[int | None]
    expires_at: NotRequired[str | None]
    """When a signed link stops working (RFC 3339); read the generation again
    for a fresh one. Absent on every other URL."""
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
    kept: NotRequired[bool | None]
    """``False`` when litegen kept no copy: the vendor's own, unconverted file."""


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
    """What the job cost: 0 once a failed or cancelled job has been refunded."""
    metadata: NotRequired[dict[str, str] | None]
    """The request's ``metadata``, as sent."""


class GenerationAsset(TypedDict):
    """One file litegen kept for a generation (``GET /v1/generations/{id}/assets``).
    ``index`` is what :meth:`download_asset` takes."""

    index: int
    kind: str
    format: str
    url: str
    expires_at: NotRequired[str | None]
    """When a signed link stops working (RFC 3339); absent on every other URL."""
    size_bytes: NotRequired[int | None]
    sha256: NotRequired[str | None]
    width: NotRequired[int | None]
    height: NotRequired[int | None]
    duration_s: NotRequired[float | None]
    fps: NotRequired[float | None]
    has_audio: NotRequired[bool | None]
    """Videos: whether the file carries an audio track (read from its bytes)."""
    kept: NotRequired[bool | None]
    """``False`` when litegen kept no copy (``keep_outputs: false``): ``url`` is
    the vendor's own file, which the vendor deletes on its own schedule."""


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
    """What litegen keeps on the row; ``metadata["request"]["metadata"]`` is the
    caller's own ``metadata``, as sent."""
    org_id: NotRequired[str | None]
    app_id: NotRequired[str | None]
    files_deleted_at: NotRequired[str | None]
    """When litegen deleted the kept files (RFC 3339): after the app's
    retention, or on request. The record stays."""
    files_deleted_reason: NotRequired[str | None]
    """``retention`` or ``deleted``."""


class CancelledGeneration(Generation):
    """What ``generations.cancel`` returns: the row as settled, and whether the
    vendor took the cancel."""

    vendor_cancelled: bool
    """``True`` when the vendor accepted it, so a queued job will not run;
    ``False`` when it has no cancel, had already finished, or did not answer
    within 5 seconds. The job is ``cancelled`` and refunded either way."""


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
    """Its media type, as the server sent it (e.g. ``"model/obj"``, or
    ``"application/zip"`` with ``package="zip"``)."""
    filename: str
    """A file name for it: the server's Content-Disposition (``model.<to>``, or
    ``model.<to>.zip`` with ``package="zip"``)."""
    format: Model3dFormat
    """The container it is in — the ``to_format`` that was asked for."""
    losses: list[str]
    """What this file lost on the way to the target container (e.g.
    ``["materials", "uvs"]``), from the ``X-Litegen-Conversion-Losses`` header.
    Worked out from the input's content, not the format pair; ``[]`` when
    nothing was lost. Known kinds: animation, skins, morph_targets, normals,
    uvs, vertex_colors, textures, image_formats, materials, alpha, hierarchy,
    meshes, texture_sampling, texture_transforms, geometry, lights, cameras,
    points, lines — a kind added later passes through as-is."""
    warnings: list[str]
    """What reading the input itself dropped or approximated, in prose, from the
    ``X-Litegen-Conversion-Warnings`` header (split on ``"; "``): a material
    library or texture the upload could not include, cubic and NURBS curves, an
    FBX animation layer. Non-ASCII characters arrive as ``?``; ``[]`` when the
    reader had nothing to say."""


# ─── Management: keys, provider configs, invitations ────────────────────────


Role = Literal["owner", "admin", "member", "viewer"]
"""A member's role in an organization. An invitation can't make an ``owner``:
the owner hands over with transfer-owner."""


class UpdateApiKeyRequest(TypedDict, total=False):
    """What ``keys.patch`` takes (``PATCH /v1/keys/{id}``): a JSON merge patch.

    A key given is set and a key left out stays as it is. ``None`` is sent as
    ``null``, which clears ``token_quota``, ``rpm_limit``, ``webhook_url`` and
    ``expires_at``; ``name``, ``scopes`` and ``is_active`` can't be cleared.
    """

    name: str | None
    token_quota: float | None
    """``None`` removes the quota."""
    rpm_limit: int | None
    """Requests per minute; ``None`` removes the limit."""
    scopes: str | None
    """Comma-separated (``"generate,read"``): replaces the key's scopes, with the
    same check as on create (an unknown scope is a 400, one the caller may not
    grant a 403)."""
    webhook_url: str | None
    """``None`` or ``""`` removes the webhook."""
    expires_at: str | None
    """RFC 3339; ``None`` removes the expiry."""
    is_active: bool | None


class ApiKeyDetail(TypedDict):
    """One key, as ``keys.get`` and ``keys.patch`` return it. Never the secret."""

    id: str
    name: str
    key_prefix: str
    created_at: str
    is_active: bool
    tokens_used: float
    scopes: str
    """Comma-separated, e.g. ``"generate,read"``."""
    token_quota: NotRequired[float | None]
    rpm_limit: NotRequired[int | None]
    webhook_url: NotRequired[str | None]
    expires_at: NotRequired[str | None]


class PatchProviderConfigRequest(TypedDict, total=False):
    """What ``admin.providers.patch`` takes (``PATCH /v1/admin/providers/{name}``).

    A key given is set, ``None`` (sent as ``null``) clears it so it falls back
    to env/YAML, and a key left out keeps its stored value. The secrets
    (``api_key``, ``api_keys``, ``key_id``, ``key_secret``) differ: left out or
    ``None``, a secret keeps its stored value, and ``""`` removes it, so the
    env/YAML one applies again. A map or ``options`` given replaces the stored
    one whole.
    """

    api_key: str | None
    api_keys: str | None
    key_id: str | None
    key_secret: str | None
    api_base: str | None
    """Sent back as ``admin.providers.get`` showed it
    (``https://[redacted]@host/v1``), it keeps the URL in force."""
    model_mapping: dict[str, str] | None
    extra_headers: dict[str, str] | None
    options: Any
    region: str | None
    enabled: bool | None
    """``False`` takes the provider out of service until re-enabled."""


class ProviderConfigView(TypedDict):
    """One provider's config, as the ``admin.providers`` methods return it, with
    every secret masked."""

    name: str
    modalities: list[str]
    """What the adapter serves: ``image``, ``video``, ``model3d``."""
    active: bool
    """Whether a live instance is registered and serving requests."""
    source: str
    """Where the config in force comes from: ``env``, ``runtime`` or ``none``."""
    has_secret: bool
    key_hint: NotRequired[str | None]
    """The key's last 4 characters, e.g. ``…a1b2``."""
    api_base: NotRequired[str | None]
    """The api_base in force, with ``[redacted]`` for its user info."""
    model_mapping: dict[str, str]
    extra_header_names: list[str]
    """Header names only: their values can carry secrets."""
    options: NotRequired[Any]
    """The options in force, with ``[redacted]`` for the user info of each URL."""
    region: NotRequired[str | None]
    enabled: bool
    updated_at: NotRequired[str | None]


class InviteResponse(TypedDict):
    """A new invitation, as ``orgs.members.invite`` returns it. Its ``token`` and
    ``invite_url`` are shown only here: litegen sends no email, so give one of
    them to the invitee."""

    id: str
    email: str
    role: str
    org_id: str
    """The organization the invitee joins on accepting."""
    expires_at: str
    token: str
    """Accepts the invitation once, until ``expires_at``
    (``POST /v1/auth/invitations/{token}/accept``): whoever holds it joins the
    org as ``role``."""
    invite_url: str | None
    """The dashboard's page that accepts it, ``/invite/{token}`` on the origin of
    the server's ``server.public_base_url``; ``None`` when that isn't set, so
    build it from ``token`` on the dashboard's origin."""


class PendingInvitation(TypedDict):
    """An invitation not yet accepted or expired (``orgs.invitations.list``),
    without its token: to send a lost link again, revoke it and invite anew."""

    id: str
    email: str
    role: str
    org_id: str
    invited_by: str | None
    """The account that sent it; ``None`` when the master key did."""
    created_at: str
    expires_at: str


class AuditLogEntry(TypedDict):
    """One audit log entry (``audit.list``): an admin action, who took it and
    what it changed."""

    id: str
    action: str
    """E.g. ``"key.create"``, ``"generation.cancel"``."""
    actor_label: str
    """Who acted: a signed-in person's email, an API key's id, ``master-key``
    for the master key, or ``system`` for litegen itself (a budget alert)."""
    actor_key_id: NotRequired[str | None]
    """The API key that acted; ``None`` for the master key, a signed-in person
    and litegen itself."""
    target_type: str
    """What the action changed: ``api_key``, ``generation``, ``user``,
    ``invitation``, ``provider``, ``settings``, a budget, …"""
    target_id: str
    before_json: NotRequired[str | None]
    """The target as JSON before the action; ``None`` on a create."""
    after_json: NotRequired[str | None]
    """The target as JSON after the action; ``None`` on a delete or revoke."""
    created_at: str
    org_id: NotRequired[str | None]
    """The organization the entry is filed under: the one whose object the
    action changed, else the one the caller acted in. Absent for an entry filed
    under no org (in hosted mode, the master key's changes to the gateway and to
    accounts, and an account change by a person in no org), which only the
    master key reads."""


class AuditLogPage(TypedDict):
    """One page of ``audit.list``."""

    data: list[AuditLogEntry]
    total: int
    page: int
    per_page: int
    total_pages: int


class WebhookDelivery(TypedDict):
    """One attempt at delivering a key's webhook (``keys.list_webhook_deliveries``)."""

    id: str
    key_id: str
    generation_id: str
    url: str
    attempt_number: int
    success: bool
    """Whether the receiver answered 2xx."""
    status_code: NotRequired[int | None]
    response_body: NotRequired[str | None]
    """The first 16 KiB of the receiver's reply."""
    error_message: NotRequired[str | None]
    payload_json: str
    """The webhook as sent, when the reader is inside the app's owner scope for
    the generation (or its files are public); for anyone else, the generation as
    stored now, presented for them, or ``{}`` once it is gone. Every attempt of
    one delivery shows the payload of its latest attempt."""
    delivery_id: NotRequired[str | None]
    """``whd-`` and a UUID: the ``X-Litegen-Delivery-Id`` the receiver got, the
    same on every attempt of one delivery; ``None`` on attempts recorded before
    migration 029."""
    created_at: str


class WebhookDeliveryPage(TypedDict):
    """One page of ``keys.list_webhook_deliveries``, newest first."""

    data: list[WebhookDelivery]
    total: int
    page: int
    per_page: int
    total_pages: int


WebhookDeliveryStatus = Literal["pending", "delivered", "failed"]
"""Where a webhook delivery stands: an attempt is to come, one was answered
2xx, or it ended without a 2xx (a 410 Gone, its last attempt failed, or it
can't be sent)."""


class WebhookDeliveryState(TypedDict):
    """A key's webhook delivery, as ``keys.redeliver_webhook_delivery`` returns
    it: one terminal webhook of a job, sent until an attempt is answered 2xx or
    its retries run out."""

    delivery_id: str
    """``whd-`` and a UUID: the ``X-Litegen-Delivery-Id`` of every attempt."""
    generation_id: str
    """The job whose webhook this is."""
    status: WebhookDeliveryStatus
    attempts: int
    """The attempts sent in the current series; a redeliver starts a new series
    at 0."""
    next_attempt_at: str
    """When the next attempt is due, while ``pending``."""
    last_status_code: NotRequired[int | None]
    """The latest attempt's HTTP status, when it got an answer."""
    last_error: NotRequired[str | None]
    """Why the latest attempt failed, or why the delivery can't be sent."""
    created_at: str
    finished_at: NotRequired[str | None]
    """When it ended (``delivered`` or ``failed``)."""
    redelivered_at: NotRequired[str | None]
    """When it was last redelivered."""


class PasswordResetLink(TypedDict):
    """A password reset link for an account, as ``users.password_reset_link``
    returns it. Shown only in this answer: litegen sends no email, so give
    ``reset_url`` or ``token`` to the account's owner."""

    user_id: str
    email: str
    token: str
    """Sets the account's password once, until ``expires_at`` (1 hour), with
    ``POST /v1/auth/password-reset/confirm``. Whoever holds it can take the
    account over, so give it only to its owner."""
    reset_url: str | None
    """The dashboard's page that sets the password, ``/reset-password/{token}``
    on the origin of the server's ``server.public_base_url``; ``None`` when
    that isn't set, so build it from ``token`` on the dashboard's origin."""
    expires_at: str


# ─── Request logs, spend logs and gateway settings ──────────────────────────


class RequestLog(TypedDict):
    """One request, as ``logs.list`` returns it."""

    id: str
    model: str
    provider: str
    status: GenerationStatus
    media_type: MediaType
    cost_usd: float
    latency_ms: int
    created_at: str
    error: NotRequired[str | None]
    metadata: NotRequired[Any]
    cache_hit: bool
    """Whether the cache answered this request (an image's ``X-Litegen-Cache:
    hit``): a hit has a row of its own, at what it was charged."""


class RequestLogPage(TypedDict):
    """One page of ``logs.list``, newest first."""

    data: list[RequestLog]
    total: int
    page: int
    per_page: int
    total_pages: int


class SpendLogEntry(TypedDict):
    """One attributed request, as ``spend.logs`` returns it."""

    id: str
    created_at: str
    model: str
    provider: str
    media_type: str
    status: str
    cost_usd: float
    latency_ms: int
    tags: list[str]
    org_id: NotRequired[str | None]
    app_id: NotRequired[str | None]
    key_id: NotRequired[str | None]
    user_id: NotRequired[str | None]
    end_user: NotRequired[str | None]
    cache_hit: bool
    """Whether the cache answered this request: a hit has a row of its own, at
    what it was charged."""


class SpendLogPage(TypedDict):
    """One page of ``spend.logs``."""

    data: list[SpendLogEntry]
    total: int
    page: int
    per_page: int


AccessMode = Literal["public", "signed", "private"]
"""Who may open an app's stored files: anyone with the link, a signed link's
holder, or a litegen key only."""

OwnerScope = Literal["key", "app", "org"]
"""Which keys a file belongs to, for ``signed`` and ``private`` files."""

SignedLinks = Literal["store", "litegen"]
"""Who signs a signed link: the store (the bucket, or litegen's local store) or
litegen (links it can revoke)."""


class GatewaySettings(TypedDict, total=False):
    """The gateway-wide settings editable at runtime, as
    ``admin.settings.update`` takes them and ``admin.settings.get`` shows them
    (``settings``). ``None``, or a key left out, means the value from config."""

    cost_markup_percent: float | None
    cache_enabled: bool | None
    cache_ttl_seconds: int | None
    cache_max_items: int | None
    cache_max_bytes: int | None
    cache_charge_hits: bool | None
    """Whether a cache hit is charged the cached answer's price (default true);
    ``False`` makes hits free. Either way the hit's log row says ``cache_hit``."""
    retry_max_retries: int | None
    retry_backoff_ms: int | None
    retry_max_backoff_ms: int | None
    vendor_timeout_seconds: int | None
    circuit_breaker_threshold: int | None
    circuit_breaker_open_for_seconds: int | None
    alert_webhook_url: str | None
    """Receives budget alerts (a soft budget crossed, a hard budget hit)."""
    default_end_user_max_budget_usd: float | None
    default_end_user_budget_duration: str | None
    storage_access_mode: AccessMode | None
    storage_access_owner_scope: OwnerScope | None
    storage_access_signed_url_ttl_seconds: int | None
    storage_access_keep_outputs: bool | None
    storage_access_retention_days: int | None
    storage_access_signed_links: SignedLinks | None
    encrypt_provider_credentials: bool | None
    """Also encrypt each ``extra_headers`` value and the user info of
    ``api_base`` and of the URLs in ``options`` at rest; needs
    ``LITEGEN__SECRETS_KEY``. Unlike the other keys, an update that leaves it
    out keeps its stored value; ``None`` or ``False`` turns it off."""


class AccessSettings(TypedDict):
    """The stored-output settings in force."""

    mode: AccessMode
    owner_scope: OwnerScope
    signed_url_ttl_seconds: int
    keep_outputs: bool
    retention_days: int
    """0 keeps files until they are deleted."""
    signed_links: SignedLinks


class EffectiveCache(TypedDict):
    """The response cache in force."""

    enabled: bool
    ttl_seconds: int
    max_items: int
    max_bytes: int
    charge_hits: bool
    """Whether a cache hit is charged the cached answer's price."""


class EffectiveRetries(TypedDict):
    """The vendor retries in force."""

    max_retries: int
    backoff_ms: int
    max_backoff_ms: int


class EffectiveBreaker(TypedDict):
    """The circuit breaker in force."""

    threshold: int
    open_for_seconds: int


class SettingRange(TypedDict):
    """The lowest and the highest value a setting takes, both allowed."""

    min: int
    max: int


class GatewaySettingLimits(TypedDict):
    """The range of each numeric gateway setting, as ``admin.settings.update``
    is held to it (a value outside is a 400 ``invalid_settings``)."""

    cost_markup_percent: SettingRange
    cache_ttl_seconds: SettingRange
    cache_max_items: SettingRange
    cache_max_bytes: SettingRange
    retry_max_retries: SettingRange
    retry_backoff_ms: SettingRange
    retry_max_backoff_ms: SettingRange
    vendor_timeout_seconds: SettingRange
    circuit_breaker_threshold: SettingRange
    circuit_breaker_open_for_seconds: SettingRange
    storage_access_signed_url_ttl_seconds: SettingRange
    storage_access_retention_days: SettingRange


class GatewaySettingsView(TypedDict):
    """What ``admin.settings.get`` and ``admin.settings.update`` return: the
    stored settings and what is in force."""

    settings: GatewaySettings
    effective_cost_markup_percent: float
    effective_storage_access: AccessSettings
    effective_cache: EffectiveCache
    cache_entries: int
    """Answers the cache holds now, on this replica."""
    effective_retries: EffectiveRetries
    effective_vendor_timeout_seconds: int
    effective_circuit_breaker: EffectiveBreaker
    limits: GatewaySettingLimits
