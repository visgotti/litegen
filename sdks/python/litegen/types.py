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

Model3dAssetKind = Literal["mesh", "texture", "preview"]
"""What a :class:`Model3dAsset` is: the mesh itself, a texture map, or a preview render."""

GenerationStatus = Literal["pending", "processing", "completed", "failed", "cancelled"]
"""Lifecycle of an async job. ``completed``, ``failed`` and ``cancelled`` are
terminal: the job never changes again, and polling stops at the first of them."""

RefImageKind = Literal["base64", "url", "blob"]
"""How :class:`ReferenceImage` ``value`` is encoded."""

CostSource = Literal["dynamic", "estimated"]
"""``dynamic``: the vendor reported the cost. ``estimated``: from LiteGen's pricing table."""

MediaType = Literal["image", "video", "model3d"]
"""The ``media_type`` filter accepted by ``client.models.list``."""


# ─── Requests ────────────────────────────────────────────────────────────────


class ReferenceImage(TypedDict):
    """An input image. On a 3D model its ``role`` selects the mode; there is no
    separate mode switch:

    * ``"init"``: image-to-3D from a single picture.
    * ``"view-front"``, ``"view-back"``, ``"view-left"``, ``"view-right"``,
      ``"view-top"``: multiview-to-3D, one image per view.

    Which roles a model takes is in its ``ref_inputs`` (``GET /v1/models/{id}``).
    Leaving ``role`` out uses the model's default role.
    """

    type: RefImageKind
    value: str
    """The image itself, encoded as ``type`` says: base64 data, a URL, or a blob reference."""
    role: NotRequired[str | None]


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


# ─── Responses ───────────────────────────────────────────────────────────────


class UsageInfo(TypedDict):
    """What a finished generation cost."""

    cost_usd: float
    """Provider cost, in US dollars."""
    tokens: int
    """The same cost in LiteGen's internal token units."""
    cost_source: CostSource


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
