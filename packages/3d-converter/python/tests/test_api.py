"""The installed wheel, on files litegen's writer and independent tools wrote."""

import struct
import subprocess
import sys

import pytest

import litegen_3d_converter as l3d
from conftest import HUMANOID_MOCK, instanced_glb, read_testdata

MEDIA = {
    "glb": "model/gltf-binary", "gltf": "model/gltf+json", "obj": "model/obj", "stl": "model/stl",
    "ply": "application/octet-stream", "3mf": "model/3mf", "dae": "model/vnd.collada+xml",
    "fbx": "application/octet-stream", "usdz": "model/vnd.usdz+zip", "usda": "model/vnd.usda",
    "usdc": "application/octet-stream",
}

LOOKS = {
    "glb": lambda d: d.startswith(b"glTF"),
    "gltf": lambda d: b'"asset"' in d,
    "obj": lambda d: b"\nv " in d,
    "stl": lambda d: len(d) == 84 + 50 * struct.unpack_from("<I", d, 80)[0],
    "ply": lambda d: d.startswith(b"ply"),
    "3mf": lambda d: d.startswith(b"PK\x03\x04"),
    "dae": lambda d: b"<COLLADA" in d,
    "fbx": lambda d: d.startswith(b"Kaydara FBX Binary"),
    "usdz": lambda d: d.startswith(b"PK\x03\x04"),
    "usda": lambda d: d.startswith(b"#usda"),
    "usdc": lambda d: d.startswith(b"PXR-USDC"),
}


def test_converts_to_every_format(skinned_glb: bytes) -> None:
    for to, looks in LOOKS.items():
        c = l3d.convert(skinned_glb, to_format=to)
        assert c.format == to
        assert len(c.files) == 1
        f = c.files[0]
        assert isinstance(f.data, bytes)
        assert looks(f.data), to
        assert (f.name, f.media_type, f.role) == (f"model.{to}", MEDIA[to], "model")


@pytest.mark.parametrize(
    ("kwargs", "code", "param"),
    [
        ({"from_format": "fbx", "to_format": "obj"}, "format_mismatch", "from_format"),
        ({"to_format": "glb", "units": "mm"}, "param_unsupported", "units"),
        ({"to_format": "stl", "from_units": "mm"}, "param_unsupported", "from_units"),
        ({"to_format": "xyz"}, "param_invalid", "to_format"),
        ({"to_format": "stl", "units": "km"}, "param_invalid", "units"),
        ({"to_format": "fbx", "fps": 0}, "param_invalid", "fps"),
        ({"to_format": "obj", "fbx_light_units": "blender"}, "param_unsupported", "fbx_light_units"),
        ({"to_format": "fbx", "usd_light_units": "blender"}, "param_unsupported", "usd_light_units"),
        ({"to_format": "fbx", "fbx_light_units": "watts"}, "param_invalid", "fbx_light_units"),
        ({"to_format": "glb", "dae_morphs": "keep"}, "param_unsupported", "dae_morphs"),
        ({"to_format": "dae", "dae_morphs": "baked"}, "param_invalid", "dae_morphs"),
        ({"to_format": "usda", "usdz_root": "usdc"}, "param_unsupported", "usdz_root"),
        ({"to_format": "usdz", "usdz_root": "usd"}, "param_invalid", "usdz_root"),
        ({"to_format": "glb", "usd_clips": "all"}, "param_unsupported", "usd_clips"),
        ({"to_format": "usdc", "usd_clips": "none"}, "param_invalid", "usd_clips"),
    ],
)
def test_errors_carry_the_api_code_and_the_python_option_name(skinned_glb: bytes, kwargs: dict, code: str, param: str | None) -> None:
    with pytest.raises(l3d.ConvertError) as caught:
        l3d.convert(skinned_glb, **kwargs)
    e = caught.value
    assert isinstance(e, ValueError)
    assert (e.code, e.param) == (code, param), str(e)
    assert str(e)


def test_error_messages_spell_python_options(skinned_glb: bytes) -> None:
    with pytest.raises(l3d.ConvertError, match=r"^`units` applies when `to_format` is stl, obj or ply"):
        l3d.convert(skinned_glb, to_format="glb", units="mm")


def test_unknown_keywords_and_missing_target_are_type_errors(skinned_glb: bytes) -> None:
    with pytest.raises(TypeError, match=r"unexpected keyword argument 'scale'"):
        l3d.convert(skinned_glb, to_format="glb", scale=2)  # type: ignore[call-arg]
    with pytest.raises(TypeError, match=r"missing 1 required keyword argument: 'to_format'"):
        l3d.convert(skinned_glb)  # type: ignore[call-arg]


def test_unrecognised_input() -> None:
    with pytest.raises(l3d.ConvertError) as caught:
        l3d.convert(b"hello", to_format="glb")
    assert caught.value.code == "format_unrecognized"


def test_model_writes_equal_convert(skinned_glb: bytes) -> None:
    m = l3d.read_model(skinned_glb)
    assert isinstance(m, l3d.Model)
    assert m.format == "glb"
    for to in ("fbx", "usdz", "obj", "stl"):
        once = l3d.convert(skinned_glb, to_format=to)
        again = m.write(to_format=to)
        assert [f.data for f in again.files] == [f.data for f in once.files], to
        assert [(l.kind, l.what, l.why) for l in again.losses] == [(l.kind, l.what, l.why) for l in once.losses], to


def test_usdc_writes_a_crate_that_reads_back_under_either_usd_name(skinned_glb: bytes) -> None:
    usdc = l3d.convert(skinned_glb, to_format="usdc").files[0]
    assert (usdc.name, usdc.media_type) == ("model.usdc", "application/octet-stream")
    assert l3d.detect_format(usdc.data) == "usdc"
    as_usdc = l3d.convert(usdc.data, from_format="usdc", to_format="glb").files[0].data
    assert l3d.convert(usdc.data, from_format="usda", to_format="glb").files[0].data == as_usdc


def test_usdz_root_picks_the_root_layer_and_usd_clips_reaches_every_usd_write(skinned_glb: bytes) -> None:
    def root(data: bytes) -> str:
        # A zip's first entry name: the local file header's name length at
        # 26, the name at 30.
        (length,) = struct.unpack_from("<H", data, 26)
        return data[30 : 30 + length].decode()

    assert root(l3d.convert(skinned_glb, to_format="usdz").files[0].data) == "model.usda"
    assert root(l3d.convert(skinned_glb, to_format="usdz", usdz_root="usda").files[0].data) == "model.usda"
    assert root(l3d.convert(skinned_glb, to_format="usdz", usdz_root=" USDC ").files[0].data) == "model.usdc"
    for to in ("usda", "usdc", "usdz"):
        for clips in ("all", "first"):
            one_shot = l3d.convert(skinned_glb, to_format=to, usd_clips=clips).files[0].data
            assert l3d.read_model(skinned_glb).write(to_format=to, usd_clips=clips).files[0].data == one_shot, (to, clips)
    with pytest.raises(l3d.ConvertError, match=r"^`usdz_root` applies when `to_format` is usdz$"):
        l3d.convert(skinned_glb, to_format="usda", usdz_root="usdc")
    with pytest.raises(l3d.ConvertError, match=r"^`usd_clips` applies when `to_format` is usda, usdc or usdz$"):
        l3d.convert(skinned_glb, to_format="glb", usd_clips="all")


def test_dae_morphs_bakes_or_keeps_the_morph_targets() -> None:
    morph = read_testdata("gltf/gltfpack/morph.glb")
    bake = l3d.convert(morph, to_format="dae")
    keep = l3d.convert(morph, to_format="dae", dae_morphs="keep")
    assert l3d.convert(morph, to_format="dae", dae_morphs="bake").files[0].data == bake.files[0].data
    assert keep.files[0].data != bake.files[0].data
    assert "morph_targets" in [l.kind for l in bake.losses]
    assert "morph_targets" not in [l.kind for l in keep.losses]
    # Case- and space-insensitive, as litegen's HTTP API takes it.
    assert l3d.convert(morph, to_format="dae", dae_morphs=" KEEP ").files[0].data == keep.files[0].data
    assert l3d.read_model(morph).write(to_format="dae", dae_morphs="keep").files[0].data == keep.files[0].data


def test_files_and_zip_input() -> None:
    fbx = read_testdata("fbx/textured_assimp_7500.fbx")
    package = l3d.convert(fbx, to_format="obj", package="files")
    roles = {f.name: f.role for f in package.files}
    assert "material_library" in roles.values() and "texture" in roles.values()
    m = l3d.read_model({f.name: f.data for f in package.files})
    assert m.format == "obj"
    assert m.info.textures >= 1
    assert {f.name: f.role for f in m.files} == roles

    zipped = l3d.convert(fbx, to_format="obj", package="zip")
    assert (zipped.files[0].name, zipped.files[0].media_type) == ("model.obj.zip", "application/zip")
    assert l3d.read_model(zipped.files[0].data).info.textures >= 1


def test_bytes_bytearray_memoryview_inputs_agree(skinned_glb: bytes) -> None:
    want = l3d.convert(skinned_glb, to_format="obj").files[0].data
    for data in (bytearray(skinned_glb), memoryview(skinned_glb), memoryview(bytearray(skinned_glb))):
        assert l3d.convert(data, to_format="obj").files[0].data == want, type(data)


def test_detect_format() -> None:
    cases = {
        "fbx/textured_assimp_7500.fbx": "fbx",
        "fbx/cube_assimp_ascii.fbx": "fbx",
        "dae/scenekit/skinned_scenekit.dae": "dae",
        "3mf/assimp_textured.3mf": "3mf",
        "usd/textured_apple.usdz": "usdz",
        "usd/cube.usdc": "usdc",
        "usd/comp.usda": "usda",
        "ply/cube_be_struct.ply": "ply",
        "fbx/skinned_small_source.glb": "glb",
    }
    for path, want in cases.items():
        assert l3d.detect_format(read_testdata(path)) == want, path
    assert l3d.detect_format(b"", "scene.usdc") == "usdc"
    assert l3d.detect_format(b"", "scene.usd") == "usda"
    assert l3d.detect_format(read_testdata("usd/cube.usdc"), "scene.usd") == "usdc"
    assert l3d.detect_format(b"hello") is None


def test_formats_limits_and_version() -> None:
    all_formats = l3d.formats()
    assert [f.format for f in all_formats] == ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc"]
    stl = next(f for f in all_formats if f.format == "stl")
    assert stl.read_options == ["from_units", "from_up"]
    assert stl.write_options == ["units", "up"]
    assert stl.stored_frame == "caller"
    fbx = next(f for f in all_formats if f.format == "fbx")
    assert fbx.read_options == ["fbx_color_space", "fbx_light_units"]
    assert fbx.write_options == ["fbx_color_space", "fbx_light_units", "fps"]
    usdz = next(f for f in all_formats if f.format == "usdz")
    assert (usdz.read_options, usdz.write_options) == (["usd_light_units"], ["usd_light_units", "usdz_root", "usd_clips", "fps"])
    for name in ("usda", "usdc"):
        usd = next(f for f in all_formats if f.format == name)
        assert (usd.read_options, usd.write_options, usd.extensions) == (["usd_light_units"], ["usd_light_units", "usd_clips", "fps"], [name, "usd"])
    dae = next(f for f in all_formats if f.format == "dae")
    assert (dae.read_options, dae.write_options) == ([], ["dae_morphs", "fps"])
    obj = next(f for f in all_formats if f.format == "obj")
    assert (obj.keeps.materials, obj.keeps_in_package.materials) == ("vertex_color", "phong")
    assert l3d.MAX_INPUT_BYTES == 134217728
    assert l3d.version().count(".") == 2


def test_info_and_roles(skinned_glb: bytes) -> None:
    m = l3d.read_model(skinned_glb)
    i = m.info
    assert (i.triangles, i.vertices, i.nodes, i.meshes, i.textures, i.uv_sets, i.vertex_colors) == (48, 35, 4, 1, 1, 1, False)
    assert i.bounds is not None
    assert (i.bounds.min, i.bounds.max) == ((-0.30000001192092896, 0.0, -0.30000001192092896), (0.30000001192092896, 3.0, 0.30000001192092896))
    assert [(x.name, x.maps) for x in i.materials] == [("tube_mat", ["base_color"])]
    assert i.skin is not None
    assert (i.skin.joints, i.skin.root, i.skin.bind_pose, i.skin.humanoid) == (3, "hip", "unknown", None)
    assert [(a.name, a.duration_s, a.root_travel_m, a.in_place) for a in i.animations] == [("Bend", 2.0, (0.0, 0.0, 0.0), True)]
    # Values that file leaves at their defaults.
    travelling = l3d.read_model(read_testdata("gltf/gltfpack/skinned.glb")).info
    assert [(a.name, a.duration_s, a.root_travel_m, a.in_place) for a in travelling.animations] == [("Bend", 2.0, (0.4000000059604645, 0.0, 0.0), False)]
    assert l3d.read_model(read_testdata("fbx/textured_assimp_7500.fbx")).info.vertex_colors is True
    assert l3d.read_model(read_testdata("gltf/gltfpack/multiuv.glb")).info.uv_sets == 4
    assert [(f.name, f.role, f.size) for f in m.files] == [("model.glb", "model", len(skinned_glb))]


def test_the_literal_aliases_exist_at_runtime() -> None:
    import typing

    assert typing.get_args(l3d.Format) == ("glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc")
    assert typing.get_args(l3d.Unit) == ("m", "cm", "mm", "in", "ft")
    assert typing.get_args(l3d.DaeMorphs) == ("bake", "keep")
    assert typing.get_args(l3d.UsdzRoot) == ("usda", "usdc")
    assert typing.get_args(l3d.UsdClips) == ("all", "first")
    assert typing.get_args(l3d.ErrorCode) == (
        "format_unrecognized", "format_mismatch", "input_ambiguous", "input_too_large", "mesh_malformed",
        "mesh_unsupported", "mesh_empty", "param_unsupported", "param_invalid",
    )
    assert {"lights", "cameras", "points", "lines"} <= set(typing.get_args(l3d.LossKind))
    stl = next(f for f in l3d.formats() if f.format == "stl")
    assert (stl.keeps.lights, stl.keeps.cameras) == (False, False)
    # STL holds triangles only.
    assert (stl.keeps.points, stl.keeps.lines) == (False, False)


def test_version_is_exported() -> None:
    assert l3d.__version__ == l3d.version()
    assert "__version__" in l3d.__all__


def test_a_lone_zip_may_carry_its_own_name() -> None:
    fbx = read_testdata("fbx/textured_assimp_7500.fbx")
    zipped = l3d.convert(fbx, to_format="obj", package="zip").files[0].data
    assert l3d.read_model(zipped, file_name="fox.zip").format == "obj"
    assert l3d.convert({"fox.zip": zipped}, to_format="glb", file_name="fox.zip").format == "glb"
    # A name inside the zip still says which file is the model.
    assert l3d.convert({"fox.zip": zipped}, to_format="glb", file_name="model.obj").format == "glb"
    with pytest.raises(l3d.ConvertError) as e:
        l3d.read_model({"fox.zip": zipped}, file_name="other.zip")
    assert (e.value.code, e.value.param) == ("param_invalid", "file_name")


def test_an_obj_whose_first_face_is_past_the_first_64_kib_is_detected() -> None:
    obj = "".join(f"v {i}.000000 0.000000 0.000000\n" for i in range(4000)).encode() + b"f 1 2 3\n"
    assert len(obj) > 64 * 1024
    assert l3d.detect_format(obj) == "obj"
    assert l3d.convert(obj, to_format="stl").format == "stl"


def test_reading_and_converting_never_measure_the_model() -> None:
    # 70 nodes place one 60,000-vertex mesh: 4.2 million vertices placed, past
    # what measuring flattens (4 million); the hosted converter converts it.
    glb = instanced_glb(70)
    assert l3d.convert(glb, to_format="glb").format == "glb"
    model = l3d.read_model(glb)
    assert model.write(to_format="gltf").format == "gltf"
    with pytest.raises(l3d.ConvertError) as e:
        _ = model.info
    assert e.value.code == "mesh_unsupported"
    assert l3d.read_model(instanced_glb(2, 10)).info.triangles == 10, "each mesh counted once"


def test_materials_name_their_maps_and_a_humanoid_rig_its_bones(skinned_glb: bytes) -> None:
    import typing

    textured = l3d.read_model(read_testdata("fbx/textured_assimp_7500.fbx")).info
    maps = [m for mat in textured.materials for m in mat.maps]
    assert "base_color" in maps
    assert set(maps) <= set(typing.get_args(l3d.TextureMap))
    skin = l3d.read_model(HUMANOID_MOCK.read_bytes()).info.skin
    assert skin is not None and skin.humanoid is not None
    assert skin.humanoid.naming == "litegen-mock"
    assert [(b.bone, b.joint) for b in skin.humanoid.bones] == [
        ("hips", "hips"), ("spine", "torso"), ("head", "head"), ("leftUpperArm", "arm_l"),
        ("rightUpperArm", "arm_r"), ("leftUpperLeg", "leg_l"), ("rightUpperLeg", "leg_r"),
    ]
    assert set(typing.get_args(l3d.VrmBone)) >= {b.bone for b in skin.humanoid.bones}
    assert "litegen-mock" in typing.get_args(l3d.HumanoidNaming)
    plain = l3d.read_model(skinned_glb).info.skin
    assert plain is not None and plain.humanoid is None


def peak_growth(setup: str, call: str) -> tuple[str | None, int]:
    """How much `call` raises a fresh interpreter's peak resident memory, in
    bytes, after `setup` ran there: a process of its own, so no other test's
    peak hides it (macOS counts ru_maxrss in bytes, Linux in KiB)."""
    script = f"""
import resource, sys
import litegen_3d_converter as l3d
def peak():
    p = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return p if sys.platform == "darwin" else p * 1024
{setup}
before = peak()
try:
    {call}
except l3d.ConvertError as e:
    print(e.code)
print(peak() - before)
"""
    out = subprocess.run([sys.executable, "-c", script], capture_output=True, text=True, check=True).stdout.split()
    return (out[0] if len(out) == 2 else None, int(out[-1]))


MIB = 1024 * 1024


def test_bytes_are_borrowed_even_in_a_mapping() -> None:
    # 60 MiB of zeros (written, so resident before measuring) under names no
    # format claims: the read fails fast (format_unrecognized), after any copy
    # the binding would make. Two of them stay under the 128 MiB cap.
    setup = 'big = b"\\0" * (60 * 1024 * 1024)'
    for call in ('l3d.convert(big, to_format="glb")', 'l3d.convert({"a.txt": big, "b.txt": big}, to_format="glb")'):
        code, growth = peak_growth(setup, call)
        assert code == "format_unrecognized", call
        assert growth < 32 * MIB, f"{call}: peak grew {growth / MIB:.0f} MiB"


def test_an_input_past_the_cap_is_refused_before_it_is_copied() -> None:
    setup = 'big = bytearray(b"\\0") * (l3d.MAX_INPUT_BYTES + 1)'
    for call in ('l3d.convert(big, to_format="glb")', 'l3d.convert(memoryview(big), to_format="glb")', 'l3d.convert({"a.glb": big}, to_format="glb")', 'l3d.detect_format(big)'):
        code, growth = peak_growth(setup, call)
        assert code == "input_too_large", call
        assert growth < 32 * MIB, f"{call}: peak grew {growth / MIB:.0f} MiB"


def test_detect_format_refuses_an_input_past_the_cap() -> None:
    with pytest.raises(l3d.ConvertError) as e:
        l3d.detect_format(bytearray(l3d.MAX_INPUT_BYTES + 1))
    assert e.value.code == "input_too_large"


def test_fps_is_a_whole_number_not_a_bool(skinned_glb: bytes) -> None:
    # Python 3.11+ appends PyO3's "while processing 'fps'" note, which match sees.
    with pytest.raises(TypeError, match=r"^`fps` takes an int, not a bool(\n|$)"):
        l3d.convert(skinned_glb, to_format="fbx", fps=True)  # type: ignore[arg-type]
    assert l3d.convert(skinned_glb, to_format="fbx", fps=24).format == "fbx"


def test_version_matches_the_installed_distribution() -> None:
    from importlib.metadata import version

    assert l3d.version() == version("litegen-3d-converter")


def test_the_wheel_is_abi3() -> None:
    from pathlib import Path

    assert [p.name for p in Path(l3d.__file__).parent.glob("*.so")] == ["litegen_3d_converter.abi3.so"]


def test_every_literal_alias_matches_the_stub() -> None:
    """stubtest skips the Literal aliases (its limitation, tests/stubtest-allowlist.txt),
    so each runtime alias is compared with the stub's here."""
    import ast
    import typing
    from pathlib import Path

    tree = ast.parse((Path(__file__).resolve().parents[1] / "litegen_3d_converter.pyi").read_text())
    stub = {}
    for n in tree.body:
        if isinstance(n, ast.AnnAssign) and isinstance(n.target, ast.Name) and isinstance(n.value, ast.Subscript) and ast.unparse(n.value.value) == "typing.Literal":
            elts = n.value.slice.elts if isinstance(n.value.slice, ast.Tuple) else [n.value.slice]
            stub[n.target.id] = tuple(e.value for e in elts if isinstance(e, ast.Constant))
    assert len(stub) == 20, sorted(stub)
    for name, values in stub.items():
        assert typing.get_args(getattr(l3d, name)) == values, name
