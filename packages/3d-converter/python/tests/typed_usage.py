# pyright: strict, reportUnnecessaryTypeIgnoreComment=true
"""Type-checked, never run (tests/test_typing.py): the valid uses below must
pass mypy --strict and pyright strict, and every line marked `type: ignore`
must be an error, or the unneeded ignore fails both."""

from __future__ import annotations

import litegen_3d_converter as l3d
from litegen_3d_converter import (
    Conversion, ConvertError, DaeMorphs, ErrorCode, Format, FormatInfo, HumanoidNaming, LossKind, Model, ModelInfo, TextureMap,
    UsdClips, UsdzRoot,
    VrmBone,
)


def valid(data: bytes) -> None:
    c: Conversion = l3d.convert(data, to_format="obj", package="zip", units="mm", up="z")
    fmt: Format = c.format
    body: bytes = c.files[0].data
    kind: LossKind | None = c.losses[0].kind if c.losses else None
    m: Model = l3d.read_model({"a.gltf": data, "a.bin": data}, from_format="gltf", from_units="cm")
    info: ModelInfo = m.info
    triangles: int = info.triangles
    seconds: float | None = info.animations[0].duration_s if info.animations else None
    w: Conversion = m.write(to_format="fbx", fbx_color_space="linear", fbx_light_units="blender", fps=24)
    usd: Conversion = l3d.convert(data, to_format="usdz", usd_light_units="photometric")
    morphs: DaeMorphs = "keep"
    dae: Conversion = m.write(to_format="dae", dae_morphs=morphs)
    baked: Conversion = l3d.convert(data, to_format="dae", dae_morphs="bake")
    root: UsdzRoot = "usdc"
    clips: UsdClips = "all"
    crate_rooted: Conversion = m.write(to_format="usdz", usdz_root=root, usd_clips="first")
    crate: Conversion = l3d.convert(data, to_format="usdc", usd_clips=clips, usd_light_units="blender")
    usdc_format: Format = "usdc"
    detected: Format | None = l3d.detect_format(data, "x.glb")
    every: list[FormatInfo] = l3d.formats()
    limit: int = l3d.MAX_INPUT_BYTES
    maps: list[TextureMap] = info.materials[0].maps if info.materials else []
    if info.skin is not None and info.skin.humanoid is not None:
        naming: HumanoidNaming = info.skin.humanoid.naming
        bones: dict[VrmBone, str] = {b.bone: b.joint for b in info.skin.humanoid.bones}
        print(naming, bones)
    version: str = l3d.__version__
    # Any buffer, as the README says: array.array, mmap, NumPy arrays.
    import array
    import mmap

    l3d.convert(array.array("B", data), to_format="stl")
    with mmap.mmap(-1, len(data)) as mapped:
        l3d.detect_format(mapped)
    try:
        l3d.convert(bytearray(data), to_format="usdz")
    except ConvertError as e:
        code: ErrorCode = e.code
        param: str | None = e.param
        print(code, param)
    print(fmt, body, kind, triangles, seconds, w, usd, dae, baked, crate_rooted, crate, usdc_format, detected, every, limit, maps, version)


def invalid(data: bytes) -> None:
    l3d.convert(data, to_format="xyz")  # type: ignore[arg-type]
    l3d.convert(data, to_format="stl", units="km")  # type: ignore[arg-type]
    l3d.convert(data)  # type: ignore[call-arg]
    l3d.convert(data, to_format="stl", scale=2)  # type: ignore[call-arg]
    l3d.convert(data, to_format="fbx", fbx_color_space="rgb")  # type: ignore[arg-type]
    l3d.convert(data, to_format="fbx", fbx_light_units="watts")  # type: ignore[arg-type]
    l3d.convert(data, to_format="dae", dae_morphs="baked")  # type: ignore[arg-type]
    l3d.convert(data, to_format="usdz", usdz_root="usd")  # type: ignore[arg-type]
    l3d.convert(data, to_format="usda", usd_clips="none")  # type: ignore[arg-type]
    l3d.convert(data, to_format="obj", package="tar")  # type: ignore[arg-type]
    text: str = l3d.convert(data, to_format="obj").files[0].data  # type: ignore[assignment]
    l3d.convert("model.glb", to_format="obj")  # type: ignore[arg-type]
    l3d.read_model(data).write(to_format="obj", from_units="mm")  # type: ignore[call-arg]
    not_a_map: TextureMap = "diffuse"  # type: ignore[assignment]
    not_a_bone: VrmBone = "leftArm"  # type: ignore[assignment]
    colour: LossKind = "colour"  # type: ignore[assignment]
    print(text, colour, not_a_map, not_a_bone)
