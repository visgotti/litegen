# litegen-3d-converter

Convert 3D models between **glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda
and usdc**, every pair, both ways, from Python. Each conversion also tells you
what the target format couldn't hold.

This is [litegen](https://github.com/visgotti/litegen)'s own converter, the one
behind its `POST /v1/utils/models3d/convert`, as a native extension. It runs
on your machine and uploads nothing.

```sh
pip install litegen-3d-converter
```

It needs Python 3.10 or later. One abi3 wheel per platform covers every
CPython from 3.10 on (abi3 has no free-threaded form, so a free-threaded
build such as 3.14t needs a wheel built for it). The package is fully typed:
a `py.typed` stub with `Literal` types for every closed set of values.

## Convert a file

```python
>>> import litegen_3d_converter as l3d
>>> obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"
>>> result = l3d.convert(obj, to_format="glb")
>>> result.files[0].name, result.files[0].media_type
('model.glb', 'model/gltf-binary')
>>> result.losses
[]

```

`convert(data, to_format=..., ...)` reads the input, writes it as
`to_format`, and returns a `Conversion`:

| attribute | what it is |
|---|---|
| `format` | the format written |
| `files` | the written files, the model first: `OutputFile` with `name`, `media_type`, `role` and `data` (`bytes`) |
| `losses` | what the target couldn't hold: `Loss` with `kind`, `what` and `why` |
| `warnings` | what reading the input dropped or approximated, in prose |

The input's format is detected from its bytes, and from `file_name` when the
bytes can't tell. Pass `from_format` to name it yourself. A USD layer is
`usda` when it is text and `usdc` when it is a binary crate, and either name
reads either.

`data` can be `bytes`, which is read in place without a copy, or any other
buffer (`bytearray`, `memoryview`, `array.array`, `mmap`, a NumPy array),
which is copied once. The stub types it as any buffer (PEP 688). The GIL is
released for the whole read and write, so threads convert in parallel. The
results are plain read-only objects that don't pickle: in a process pool,
return `files[0].data`, which is `bytes`.

## Several files, or a zip

Some formats keep their parts beside the model: a `.gltf` its `.bin` and
images, an OBJ its `.mtl` and textures, Collada, USDA and USDC their textures. Pass
all of them as a mapping of file names to bytes. The converter finds the
model and resolves its references, first by relative path, then by file name
ignoring case, which is how a flat folder of files arrives. A zip of such a
package (as `package="zip"` writes) is unpacked.

```python
>>> obj = b"mtllib cube.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"
>>> mtl = b"newmtl red\nKd 1 0 0\n"
>>> model = l3d.read_model({"cube.obj": obj, "cube.mtl": mtl})
>>> model.format, [(f.name, f.role) for f in model.files]
('obj', [('cube.obj', 'model'), ('cube.mtl', 'material_library')])

```

To write a package yourself, set `package`:

| `package` | you get |
|---|---|
| `"file"` (default) | one self-contained file. An OBJ's materials become vertex colours; Collada, USDA and USDC leave textures out (reported as losses) |
| `"zip"` | one zip of the model and the files it references (`model.<ext>.zip`) |
| `"files"` | those files separately, the model first, named as the model references them |

## Read once, write many

`read_model` parses once. `model.write(to_format=...)` then writes any format
from the same read, which is faster than calling `convert` per format.

```python
>>> model = l3d.read_model(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n")
>>> model.info.triangles
1
>>> [model.write(to_format=f).files[0].name for f in ("stl", "ply", "usdz")]
['model.stl', 'model.ply', 'model.usdz']

```

`model.info` is what the converter measures in the model: triangles,
vertices, nodes, meshes, textures, uv sets, vertex colour, bounds in metres,
materials and the maps each uses, the skin, and every animation clip. A clip
has its duration in seconds, its root travel in metres, and whether it stays
in place. The skin has its joints, root and bind pose. When the joints are
named in a scheme litegen knows (VRM, Unity, Mixamo, Meshy), the skin also
says which joint is which VRM humanoid bone.

Reading never measures. `info` measures on first use, placing every instance
of every mesh to do it. A model past 4,000,000 vertices once placed raises
`ConvertError` (`mesh_unsupported`) there, and still converts. Triangle and
vertex counts are per mesh: a mesh that several nodes place counts once.

## Options

The names match litegen's Python SDK (`client.models3d.convert`).

| option | for | what it does |
|---|---|---|
| `to_format` | always | the format to write |
| `from_format` | reading | the input's format; detected when absent |
| `file_name` | reading | the input's name, for detection; or, with several files, which one is the model |
| `package` | writing | `"file"`, `"zip"` or `"files"` (above) |
| `units`, `up` | writing stl, obj, ply | the unit the numbers are written in (`m`, `cm`, `mm`, `in`, `ft`) and the up axis (`y`, `z`). These formats carry no units or axis of their own; slicers read STL as millimetres |
| `from_units`, `from_up` | reading stl, obj, ply | the same, for the input |
| `fbx_color_space` | an fbx at either end | how FBX material colours are stored: `"srgb"` (default; three.js reads them so) or `"linear"` (Blender, Unity, Unreal, ufbx, assimp) |
| `fbx_light_units` | an fbx at either end | how FBX light intensities are stored: `"photometric"` (default; glTF's candela and lux, as ufbx and three.js read them) or `"blender"` (Blender's watts: the file opens in Blender as bright as the scene's glTF) |
| `usd_light_units` | a usdz, usda or usdc at either end | the same, for USD lights |
| `dae_morphs` | writing dae | how morph targets (blend shapes) are written: `"bake"` (default; each node's mesh at that node's weights, which every Collada reader opens, with the targets and their weight animation reported as losses) or `"keep"` (`<morph>` controllers with their weight animation, which SceneKit (and so Xcode) and Godot read, while three.js's ColladaLoader and assimp refuse the whole file) |
| `usdz_root` | writing usdz | what the package's root layer, its first entry, is: `"usda"` (default; USDA text, which `usdchecker --arkit` accepts and `<model-viewer>` hands to AR Quick Look as it is) or `"usdc"` (the binary crate, as Apple's own packagers write it). The two read back as one scene |
| `usd_clips` | writing usda, usdc, usdz | which animation clips the file holds: `"all"` (every clip as an alternative, a variant set `Animation` on the default prim with one variant per clip, the first selected, so a reader that composes variants can switch between them and one that does not plays the first; rigid (node) motion is the first clip's only, the others' is named in the losses, and a scene whose clips are all rigid writes as `"first"`) or `"first"` (the first clip alone). Absent, `"all"` for usda and usdc, and `"first"` for usdz until AR Quick Look's behaviour with variants is checked on a device |
| `fps` | writing fbx, dae, usdz, usda, usdc | the rate animation is keyed at; 30 when absent |

Every option is keyword-only. Values are read as litegen's HTTP API reads
them: case and surrounding spaces don't matter, and `up` also takes `+y` and
`+z`. An option that doesn't apply to the formats at hand is an error, so a
mistake shows instead of being ignored. `formats()`
lists, for every format, the options it takes (`read_options`,
`write_options`) and what it keeps, as one file (`keeps`) and as a package
(`keeps_in_package`).

```python
>>> try:
...     l3d.convert(obj, to_format="glb", units="mm")
... except l3d.ConvertError as e:
...     print(e.code, e.param)
param_unsupported units

```

## Errors

Every function raises `ConvertError`, a `ValueError`, with a `code` (spelled
as litegen's HTTP API spells it) and, when there is one, the argument it is
about in `param`.

| `code` | when |
|---|---|
| `format_unrecognized` | no `from_format`, and neither the bytes nor the name say what the input is; an empty file; or no model among several files |
| `format_mismatch` | the bytes are not the format `from_format` says (or the one their name suggested); `param` is `from_format` |
| `input_ambiguous` | several files could be the model: pass `file_name` |
| `input_too_large` | over `MAX_INPUT_BYTES` (128 MiB) |
| `mesh_malformed` | the file is that format, but broken |
| `mesh_unsupported` | valid, but past one of the converter's limits, or using something it does not model |
| `mesh_empty` | the model has no triangles, lines or points |
| `param_unsupported` | an option that doesn't apply to these formats |
| `param_invalid` | an option's value is not one it takes |

A missing `to_format` or an unknown keyword is a `TypeError`, as for any
Python function.

## Other functions

- `detect_format(data, file_name=None)`: the format of a file, or `None`.
  Past `MAX_INPUT_BYTES` it raises `input_too_large`, as every function does.
- `formats()`: every format's name, extensions, media type, what it keeps,
  which options it takes, and the frame its numbers are stored in.
- `MAX_INPUT_BYTES`: the largest input read, 134,217,728 bytes.
- `version()` and `__version__`: this package's version.

## Same results as litegen's API

The package runs litegen's converter pipeline itself, built from the same
source with the same dependency versions. On one platform, a file converts
to the same bytes as the hosted converter's (checked on 2026-11-05: 180 of
180 sample conversions on macOS). Across platforms, a written float (an FBX
rotation, a resampled key) can differ in its last bit, because each platform
has its own maths library.

## License

MIT
