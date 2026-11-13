# @litegen/3d-converter

Convert 3D models between **glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda
and usdc**, every pair, both ways, in Node.js and in the browser. Each conversion
also tells you what the target format couldn't hold.

This is [litegen](https://github.com/visgotti/litegen)'s own converter, the one
behind its `POST /v1/utils/models3d/convert`, compiled to WebAssembly. Nothing
is uploaded anywhere, and there is no native dependency or install script.

```sh
npm install @litegen/3d-converter
```

Node 22 or later, or any browser with WebAssembly (every current one). ESM
only. Every option, result and error code is typed, with docs your editor
shows.

## Convert a file

```js
import { convert } from '@litegen/3d-converter';

// A triangle, as OBJ text.
const obj = new TextEncoder().encode('v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n');

const { files, losses } = convert(obj, { to: 'glb' });
console.log(files[0].name, files[0].mediaType, files[0].data.length > 0);
// model.glb model/gltf-binary true
console.log(losses.length);
// 0
```

`convert(input, options)` reads the input, writes it in `options.to`, and
returns a `Conversion`:

| field | what it is |
|---|---|
| `format` | the format written |
| `files` | the written files, the model first: `{ name, mediaType, role, data }`, `data` a `Uint8Array` |
| `losses` | what the target couldn't hold: `{ kind, what, why }` |
| `warnings` | what reading the input dropped or approximated, in prose |

The input's format is detected from its bytes, and from `fileName` when the
bytes can't tell. Pass `from` to say it yourself. A USD layer is `usda` when
it is text and `usdc` when it is a binary crate, and either name reads either.

## Several files, or a zip

Some formats keep their parts beside the model: a `.gltf` its `.bin` and
images, an OBJ its `.mtl` and textures, Collada, USDA and USDC their textures. Pass
all of them as an object of file names to bytes. The converter finds the model
and resolves its references, first by relative path, then by file name
ignoring case, which is how a flat drop of files arrives. A zip of such a
package (as `package: 'zip'` writes) is unpacked.

```js
import { convert, readModel } from '@litegen/3d-converter';

const obj = new TextEncoder().encode('mtllib cube.mtl\nusemtl red\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n');
const mtl = new TextEncoder().encode('newmtl red\nKd 1 0 0\n');

const model = readModel({ 'cube.obj': obj, 'cube.mtl': mtl });
console.log(model.format, model.files.map((f) => `${f.name}: ${f.role}`).join(', '));
// obj cube.obj: model, cube.mtl: material_library
model.free();
```

To write a package yourself, set `package`:

| `package` | you get |
|---|---|
| `'file'` (default) | one self-contained file. An OBJ's materials become vertex colours; Collada, USDA and USDC leave textures out (reported as losses) |
| `'zip'` | one zip of the model and the files it references (`model.<ext>.zip`) |
| `'files'` | those files separately, the model first, named as the model references them |

## Read once, write many

`readModel` parses once. `model.write(options)` then writes any format from
the same read, which is faster than calling `convert` per format.

```js
import { readModel } from '@litegen/3d-converter';

const obj = new TextEncoder().encode('v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n');
using model = readModel(obj);
console.log(model.info.triangles);
// 1
for (const to of ['stl', 'ply', 'usdz']) console.log(model.write({ to }).files[0].name);
// model.stl
// model.ply
// model.usdz
```

The model lives in WebAssembly memory. Free it with `model.free()`, or let a
`using` declaration free it at the end of its block, as above (`using` needs
Node 24, a current browser, or TypeScript 5.2 to compile it). Freeing twice
is harmless; a freed model's methods throw a `TypeError` that says so.

`model.info` is what the converter measures in the model: triangles,
vertices, nodes, meshes, textures, uv sets, vertex colour, bounds in metres,
materials and the maps each uses, the skin, and every animation clip. A clip
has its duration in seconds, its root travel in metres, and whether it stays
in place. The skin has its joints, root and bind pose. When the joints are
named in a scheme litegen knows (VRM, Unity, Mixamo, Meshy), the skin also
says which joint is which VRM humanoid bone.

Reading never measures. `info` measures on first use, placing every instance
of every mesh to do it. A model past 4,000,000 vertices once placed throws
`ConvertError` (`mesh_unsupported`) there, and still converts. Triangle and
vertex counts are per mesh: a mesh that several nodes place counts once.

## Options

The names match litegen's TypeScript SDK (`client.models3d.convert`).

| option | for | what it does |
|---|---|---|
| `to` | always | the format to write |
| `from` | reading | the input's format; detected when absent |
| `fileName` | reading | the input's name, for detection; or, with several files, which one is the model |
| `package` | writing | `'file'`, `'zip'` or `'files'` (above) |
| `units`, `up` | writing stl, obj, ply | the unit the numbers are written in (`m`, `cm`, `mm`, `in`, `ft`) and the up axis (`y`, `z`). These formats carry no units or axis of their own; slicers read STL as millimetres |
| `fromUnits`, `fromUp` | reading stl, obj, ply | the same, for the input |
| `fbxColorSpace` | an fbx at either end | how FBX material colours are stored: `'srgb'` (default; three.js reads them so) or `'linear'` (Blender, Unity, Unreal, ufbx, assimp) |
| `fbxLightUnits` | an fbx at either end | how FBX light intensities are stored: `'photometric'` (default; glTF's candela and lux, as ufbx and three.js read them) or `'blender'` (Blender's watts: the file opens in Blender as bright as the scene's glTF) |
| `usdLightUnits` | a usdz, usda or usdc at either end | the same, for USD lights |
| `daeMorphs` | writing dae | how morph targets (blend shapes) are written: `'bake'` (default; each node's mesh at that node's weights, which every Collada reader opens, with the targets and their weight animation reported as losses) or `'keep'` (`<morph>` controllers with their weight animation, which SceneKit (and so Xcode) and Godot read, while three.js's ColladaLoader and assimp refuse the whole file) |
| `usdzRoot` | writing usdz | what the package's root layer, its first entry, is: `'usda'` (default; USDA text, which `usdchecker --arkit` accepts and `<model-viewer>` hands to AR Quick Look as it is) or `'usdc'` (the binary crate, as Apple's own packagers write it). The two read back as one scene |
| `usdClips` | writing usda, usdc, usdz | which animation clips the file holds: `'all'` (every clip as an alternative, a variant set `Animation` on the default prim with one variant per clip, the first selected, so a reader that composes variants can switch between them and one that does not plays the first; rigid (node) motion is the first clip's only, the others' is named in the losses, and a scene whose clips are all rigid writes as `'first'`) or `'first'` (the first clip alone). Absent, `'all'` for usda and usdc, and `'first'` for usdz until AR Quick Look's behaviour with variants is checked on a device |
| `fps` | writing fbx, dae, usdz, usda, usdc | the rate animation is keyed at; 30 when absent |

Values are exactly as the types spell them. An option that doesn't apply to
the formats at hand is an error, not silently ignored, so a mistake shows. `formats()` lists, for every format,
the options it takes (`readOptions`, `writeOptions`) and what it keeps, as one
file (`keeps`) and as a package (`keepsInPackage`).

```js
import { convert, ConvertError } from '@litegen/3d-converter';

const obj = new TextEncoder().encode('v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n');
try {
  convert(obj, { to: 'glb', units: 'mm' });
} catch (e) {
  if (!(e instanceof ConvertError)) throw e;
  console.log(e.code, e.param);
  // param_unsupported units
  console.log(e.message);
  // `units` applies when `to` is stl, obj or ply, which carry no units or up axis; glb declares its own
}
```

## Errors

Every function throws `ConvertError`, a real `Error` subclass with a `code`
(spelled as litegen's HTTP API spells it) and, when there is one, the
option it is about in `param`.

| `code` | when |
|---|---|
| `format_unrecognized` | no `from`, and neither the bytes nor the name say what the input is; an empty file; or no model among several files |
| `format_mismatch` | the bytes are not the format `from` says (or the one their name suggested); `param` is `from` |
| `input_ambiguous` | several files could be the model: pass `fileName` |
| `input_too_large` | over `maxInputBytes()` (128 MiB) |
| `mesh_malformed` | the file is that format, but broken |
| `mesh_unsupported` | valid, but past one of the converter's limits, or using something it does not model |
| `mesh_empty` | the model has no triangles, lines or points |
| `param_unsupported` | an option that doesn't apply to these formats, or doesn't exist |
| `param_invalid` | an option's value is not one it takes |
| `param_required` | `to` is missing |

## Other functions

- `detectFormat(bytes, fileName?)`: the format of a file, or `undefined`.
  Past `maxInputBytes()` it throws `input_too_large`, as every function does.
- `formats()`: every format's name, extensions, media type, what it keeps,
  which options it takes, and the frame its numbers are stored in.
- `maxInputBytes()`: the largest input read, 134,217,728 bytes.
- `version()`: this package's version.

## Bundlers and workers

Importing the package instantiates its WebAssembly before the import
resolves: synchronously from disk in Node, and with a top-level `await` in
browsers. So every function is ready to call. In a browser the `.wasm` is
found through `new URL('litegen_3d_converter_bg.wasm', import.meta.url)`:

- **Vite and webpack 5** see that URL and emit the `.wasm` as an asset beside
  the bundle; nothing to do.
- **esbuild, Rollup and other bundlers** leave the URL as it is, so the
  `.wasm` must sit beside the bundled JavaScript that imports the package.
  Copy it there as a build step; the package exports it as
  `@litegen/3d-converter/litegen_3d_converter_bg.wasm`
  (`node_modules/@litegen/3d-converter/dist/litegen_3d_converter_bg.wasm`).

With Vite:

- workers that import the package need `worker: { format: 'es' }`, because
  classic workers can't use a top-level `await`;
- add `optimizeDeps: { exclude: ['@litegen/3d-converter'] }`, because
  pre-bundling would move the module away from its `.wasm`.

A conversion runs on the thread that calls it. For a big model in a page,
call it from a Web Worker, as the litegen browser tool does
(`apps/3d-converter` in the litegen repository).

## Same results as litegen's API

The package runs litegen's converter pipeline itself, built from the same
source with the same dependency versions, so a file converts to the same
bytes as with the hosted converter, with one exception. A float that goes
through a transcendental function (an FBX rotation's Euler angles, a
resampled key) can differ in its last bit, because WebAssembly brings its own
maths library. Checked on 2026-11-05: of 180 sample conversions, every one
matched except the six FBX writes, which differ in one to four doubles by one
unit in the last place.

## How it is built

From Rust: `litegen-core/src/mesh/` compiled as a crate, a typed core API,
and a wasm-bindgen binding whose TypeScript declarations (with their docs)
tsify generates from the Rust types. The build uses `opt-level = 3`, fat LTO,
one codegen unit and `wasm-opt -O3`, without SIMD. That was the fastest of the
eight variants measured; the rule was to take a variant within 5% of it only
if it were 15% smaller, and none was:

| variant | brotli | time |
|---|---|---|
| opt-level 3, `-O3` (this build) | 882 KiB | 826 ms |
| opt-level 3, `-O3`, simd128 | 891 KiB | 849 ms |
| opt-level s, `-Oz`, simd128 | 729 KiB | 938 ms |

(The time is `scripts/bench.mjs`, measured in Node: three real models into all
ten formats, plus a 520,000-triangle mesh into seven.)

Those sizes are from 2026-11-05. The build has grown since, with litegen's
converter: matching file names across Unicode forms, decoding Draco- and
meshopt-compressed glTF, and writing Collada morph controllers and USDC
crates with animation clip variants, among others, bring it to 1157 KiB
brotli.

## License

MIT
