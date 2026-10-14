# FBX 7.4/7.5 binary: implementation reference for a from-scratch Rust reader/writer

> Kept in the repo since 2026-10-12. Paths below that start `scratchpad/fbx/` or `scratchpad/usd3mfdae/` now live under `docs/superpowers/research/fbx/` and `usd3mfdae/`; only our scripts and hand-written inputs were kept (see [README](README.md)).

Research date: 2026-10-12. Scope: binary FBX 7400/7500 written without the Autodesk FBX SDK. The output must load in
SDK-based tools (Maya, 3ds Max, Unity, Unreal), Blender, Assimp and three.js. glTF/GLB is the hub format.

How each claim is sourced. **[V]** means I verified it against real bytes here (see "Evidence" below).
**[3js]** comes from three.js r183 `FBXLoader.js` (MIT, local copy: `dashboard/node_modules/three/examples/jsm/loaders/FBXLoader.js`).
**[assimp]** comes from Assimp 6.0 output or Assimp's BSD-3 source. **[SDK-doc]** comes from Autodesk's FBX SDK reference.
**[ufbx]** comes from ufbx (MIT/public domain). **UNVERIFIED** means no primary or high-quality confirmation.

## Evidence used (reproducible, in `scratchpad/fbx/`)
- `scripts/fbxdump.py`, `fbxtree.py`, `fbxstats.py`, `fbxconn.py`, `fbxptypes.py` are my own byte-level FBX dumpers.
- I dumped real **SDK-written** binaries from the three.js examples repo (`examples/models/fbx/*` at github.com/mrdoob/three.js, dev branch):
  - `Samba Dancing.fbx`: Mixamo, "FBX SDK/FBX Plugins version 2014.2.1", 7400.
  - `mixamo.fbx`: "FBX SDK/FBX Plugins version 2020.2", **7700**.
  - `morph-translation.fbx`: 3ds Max, SDK 2020.3.4, 7400, with a Physical material, morphs and embedded textures.
  - `exampleWindow.fbx`: 3ds Max, SDK 2020.3.5, 7400.
  - `ball_anims_asc_2018.fbx`: Maya 2018, **ASCII** 7500, with the SDK's own comments on KeyAttrFlags.
  - Blender-written files (Blender 3.6 to 4.4): `RotationTest`, `morph_test`, `vCube`, `monkey`, `monkey_embedded_texture`.
- `out/`: Assimp 6.0 `assimp export scene.gltf x.fbx` (binary) and `-ffbxa` (ASCII), run on my own glTF test scene
  (PBR material, skin, morphs, animation).
- `probe/`: files from my own minimal Python writer (`scripts/minifbx.py`). They were loaded with Assimp 6.0
  (`assimp info`) and with three.js r183 FBXLoader under Node (`node/load3.mjs`, `node/skin3.mjs`). They show what each of
  those two readers actually requires.
- Blender-written skinned files `huesitos.fbx` and `animation_with_skeleton.fbx` from Assimp's test models, and the
  ufbx_write source (MIT/Unlicense, https://github.com/ufbx/ufbx-write), read for the FileId/footer hash and key flags.
- Five research sub-agents covered:
  - the Autodesk FBX SDK 2019/2020 reference (help.autodesk.com cloudhelp);
  - ufbx, OpenFBX, Assimp and fbxcel sources, plus an agent byte-probe of 11 SDK binaries from ufbx's test corpus;
  - licensing;
  - vendor documentation;
  - PBR and unit conventions.

  Their claims carry the source they cited.

---------------------------------------------------------------------------------------------------------------------

## Summary: the facts that matter most

1. **Header.** The header is 27 bytes: `"Kaydara FBX Binary  "` + `00 1A 00` + u32 LE version. Node records use u32 headers when
   version < 7500 and u64 headers when version >= 7500. The NULL record is 13 bytes for < 7500 and 25 bytes for >= 7500. The top level ends with a NULL record. **[V]**
2. **When to write a NULL record.** Write one after a record's children. Also write one for `References` and for an empty
   `Properties70`. **Always** write one for `AnimationLayer` and `AnimationStack`, even with no children. In my probe,
   Assimp 6.0 silently dropped all animation when `AnimationLayer` had no NULL record. **[V]**
3. **Footer.** SDK files use this layout: 16-byte footer id, then 1–16 zero bytes so the next field starts on a 16-byte
   boundary, then 4 zero bytes, u32 version, 120 zero bytes, and `F8 5A 8C 6A DE F5 D9 7E EC E9 0C E3 75 8F 29 0B`. The
   file size ends up a multiple of 16. **[V]**
   - **The FileId and the footer id are hashes of the `CreationTime` string.** I checked the derivation, from MIT-licensed
     ufbx_write, against 4 SDK files **[V]**.
   - The SDK tolerates padding differences: Blender pads differently and its files load. But it **probably rejects an
     inconsistent FileId / CreationTime / footer-id triple** (ufbx issue #229).
   - Use the known-good constant triple from Blender and Assimp: FileId `28 B3 2A EB B6 24 CC C2 BF C8 B0 2A A9 2B FC F1`,
     CreationTime `"1970-01-01 10:00:00:000"`, footer id `FA BC AB 09 D0 C8 D4 66 B1 76 FB 83 1C F7 26 7E`.
4. **Array compression.** Compressed arrays (`Encoding=1`) are **zlib-wrapped** deflate (RFC 1950). All SDK files use the
   header `78 01`. Raw deflate would break three.js (`unzlibSync`). The SDK compresses arrays of about 1 KB and larger. **[V]**
5. **IDs.** Objects are identified by int64 IDs, and `0` is the scene root. Keep IDs unique, positive and **below 2^53**,
   because three.js decodes int64 into a JS Number. Names are written `"Name\x00\x01Class"`, for example
   `"Hips\x00\x01Model"`. **[V][3js]**
6. **Mapping strings.** Use `"ByVertice"` for per-control-point data, **not** `"ByControlPoint"`. Assimp 6.0 dropped normals
   and UVs with "ByControlPoint", and three.js produced garbage. The other values are `"ByPolygonVertex"`, `"ByPolygon"` and
   `"AllSame"`. **[V]**
7. **UVs.** FBX UVs are V-up, with the origin at bottom-left (OpenGL/OBJ convention). glTF→FBX is `v' = 1 - v`. **[V]**
8. **Cluster matrices.** The FILE stores `TransformLink = bone global matrix at bind time`. It stores
   `Transform = TransformLink^-1 x mesh-node global matrix at bind time`, which equals glTF's `inverseBindMatrix` when
   the FBX mesh node's global matrix equals glTF's mesh space. Matrices are 16 doubles, column-major. **[V]**
   - Verified numerically on Mixamo SDK output, on two Blender files with offset meshes, and (by an agent) on an SDK file
     with an offset mesh.
   - The SDK *API* presents Transform differently; the file convention above is what matters.
   - Write a **BindPose** containing the mesh and every bone. three.js mis-skins a mesh that is not at the origin when the
     BindPose is missing **[V]**.
9. **Animation.** `KeyTime` is int64 at 46,186,158,000 ticks per second for every file we write (7400/7500). **[V][3js]**
   **Reader caveat [SDK-doc][ufbx]:** SDK 2019.5 introduced an opt-in time base of **141,120,000** ticks per second. It is
   signalled by `FBXHeaderExtension{FBXHeaderVersion ≥ 1004, OtherFlags{TCDefinition}}`: 127 means the legacy
   46,186,158,000/s, and any other value means 141,120,000/s (ufbx). Mixamo's 7700 output says 127 **[V]**.
   Rotation curves are Euler **degrees** in the node's `RotationOrder`. three.js needs all three `d|X/d|Y/d|Z` curves of an
   `R` node to share identical key times. The safest output is baked (resampled) linear keys, because three.js ignores
   `KeyAttrFlags`. **[3js]**
10. **Required by readers.** three.js crashes on a Material without `ShadingModel`, a Texture without `FileName`, or a
    BlendShapeChannel without `FullWeights`. **[V][3js]** Assimp refuses a file without `FBXHeaderExtension`. **[V]**
    Write the full SDK-style skeleton anyway; it is cheap.
11. **Axes and units.** glTF's frame (Y-up, +Z front, right-handed) **is** the FBX "Maya Y-up" frame: `UpAxis=1`,
    `FrontAxis=2`, `CoordAxis=0`, all signs +1. Units: `UnitScaleFactor` is centimetres per file unit. Recommended
    default: metres plus `UnitScaleFactor=100`. three.js and Assimp ignore the unit, so the raw values stay equal to the
    glTF's. Offer a cm profile for Unreal/Maya (§8).
12. **Geometry data that readers silently drop.**
    - Assimp needs the `Layer`/`LayerElement` index and a `LayerElementMaterial`.
    - OpenFBX aborts on unknown mapping or reference strings.
    - three.js uses only the first Normal and Color elements.
    - Write `Layer 0` (plus `Layer n` per extra UV set) and always a material layer.
13. **Materials.** FBX has no PBR standard. Write a full Phong material using Blender's deterministic encoding
    (`ShininessExponent=(10(1−r))²`, `ReflectionFactor=metallic`, `TransparencyFactor=1−α` + `Opacity=α`). Add the
    Stingray `Maya|…` properties and `Maya|TEX_*_map` texture links for importers that read real PBR (Assimp; three.js AO).
    Vertex colours are sRGB in FBX.

---------------------------------------------------------------------------------------------------------------------

## 1. Binary container

### 1.1 Header (27 bytes) [V]
| offset | bytes | value |
|---|---|---|
| 0  | 21 | `4B 61 79 64 61 72 61 20 46 42 58 20 42 69 6E 61 72 79 20 20 00` = `"Kaydara FBX Binary  \0"` (two spaces) |
| 21 | 2  | `1A 00` |
| 23 | 4  | u32 LE version: 7400 = FBX 2014/2015, 7500 = FBX 2016–2018, 7700 = FBX 2019+ (e.g. `mixamo.fbx`, SDK 2020.2) |

three.js only checks the first 21 bytes plus `\0`, then reads the version at offset 23 and accepts any version from 6400 up [3js L3512-3523, L4088-4094].

**Version numbers [SDK-doc]** come from the FBX SDK 2020 `fbxio.h`
(https://help.autodesk.com/cloudhelp/2020/ENU/FBX-API-Reference/cpp_ref/fbxio_8h_source.html):
- `7400`: "guarantee compatibility with Autodesk 2014/2015 products"
- `7500`: "…2016/2017/2018 products"
- `7700`: "…2019/2020 products"
- `7600`: exists only for "the few existing files" written with the new time base and no flag (see §7)

The SDK 2020 default export is 7700; SDK 2016–2019.0 defaulted to 7500. The SDK 2016 release notes on 7.5 say:
*"This new file format is only compatible with the 2016 or later product versions that use 64-bit integers in the
binary block headers for large file support (> 2GB). The new file format cannot be imported in 2015 or older product
versions."* The compatibility strings map approximately as follows (inferred from the product years):

| compatibility string | file version |
|---|---|
| "FBX201400" | 7400 |
| "FBX201600", "FBX201800" | 7500 |
| "FBX201900", "FBX202000" | 7700 (SDK ≥ 2019.5) |

Autodesk's guide says: *"The FBX file format is not documented. Applications should use the FBX SDK to export and import
scene data to and from FBX files"* (https://help.autodesk.com/cloudhelp/2019/ENU/FBX-Developer-Help/welcome_to_the_fbx_sdk/what_is_autodesk_fbx_technology.html).

### 1.2 Node record [V]
| field | < 7500 | >= 7500 | meaning |
|---|---|---|---|
| EndOffset | u32 | u64 | **absolute** file offset of the first byte after this record (after its children and NULL terminator) |
| NumProperties | u32 | u64 | number of properties |
| PropertyListLen | u32 | u64 | byte length of the property list |
| NameLen | u8 | u8 | |
| Name | NameLen bytes | same | ASCII, no terminator |
| Properties | PropertyListLen bytes | same | written back to back, no padding |
| Nested records | until EndOffset−NULL | same | children |
| NULL record | 13 zero bytes | 25 zero bytes | terminates the nested list (rules below) |

A reader detects children by checking `cursor < EndOffset` after the properties. A record whose EndOffset field is 0 is a
NULL record. three.js does exactly this [3js L3562-3612]. The top-level list is the sequence of top-level records followed
by one NULL record.

Sources: Blender developer blog spec (A. Gessler, 2013), https://code.blender.org/2013/08/fbx-binary-file-format-specification/.
The 7.5 change to u64 fields and the 25-byte NULL record is described in that page's 2018 reader comment and confirmed in
bytes here. Autodesk's own note on 7.5: *"This new file format is only compatible with the 2016 or later product versions."*

**When the SDK writes a NULL record.** I measured this over all nodes of four SDK files (2014.2.1, 2020.2, 2020.3.4,
2020.3.5) **[V]**:
- A NULL record is written after the children of **every** record that has children.
- A NULL record is written for records with **0 properties and 0 children**: `References:` and an empty `Properties70:`.
- A NULL record is written for `AnimationLayer`, which has 3 properties and no children. This was seen in both Mixamo files.
- **No** NULL record is written for any other leaf record that has properties.

Assimp's exporter forces the sentinel with the flag `force_has_children`, on `AnimationStack` and `AnimationLayer`
(L2446-2463, comment "this node absurdly always pretends it has children") and on the empty `References` (L524). Source is
`code/AssetLib/FBX/FBXExporter.cpp` (master, BSD-3).
**Probe result:** without the sentinel on `AnimationLayer`, `assimp info` reports `Animations: 0`; with it, `Animations: 1`.
Assimp's reader requires a "scope" for `AnimationLayer` objects. ufbx_write writes the NULL record whenever it closes an
opened scope (`ufbxwi_binary_dom_close`) and writes leaf values without one, so every object and every
`{ }`-block gets a terminator.

Other implementations agree:
- ufbx's `transmute_fbx.py` uses `children or name in {References, AnimationStack, AnimationLayer}`.
- fbxcel uses `has_child || num_attributes == 0`.
- One SDK exception, from the agent's survey of 11 more SDK files: a zero-property, childless `Video/Content` record has
  **no** NULL record.

**Rule to implement:**
`null = has_children || name in {"References","AnimationStack","AnimationLayer"} || (name == "Properties70" && empty)`.
Never emit any other zero-property leaf record. Readers tolerate a missing NULL on nested childless records (ufbx,
Assimp and OpenFBX stop at `EndOffset`) and an extra one (fbxcel only warns). Assimp is the exception: it needs the
`AnimationLayer` one, as the probe above shows.

### 1.3 Property type codes [V]
| code | payload | notes |
|---|---|---|
| `Y` | i16 | used e.g. for `P: "filmboxTypeID","Short",…` values (`YYY`) |
| `C` | u8 | 1-byte "bool". In 7.x SDK files `Model.Shading` is its only use: `'T'` (0x54) in 11 SDK 2019–2020 files and in 3ds Max output, `'Y'` (0x59) in Mixamo **[V]**. Blender and Assimp write 0x01. three.js tests the LSB, so `'T'` reads as false [3js L3863-3869]. fbxcel claims 0x00/0x01 are "ignored by official SDK and tools" (UNVERIFIED). Write `'T'` to match the SDK; the value has no visible effect |
| `I` | i32 | |
| `F` | f32 | e.g. 3ds Max `"Float"` properties |
| `D` | f64 | |
| `L` | i64 | IDs, `KTime` |
| `f` `d` `l` `i` `b` | array: u32 ArrayLength (elements), u32 Encoding, u32 CompressedLength (bytes of payload) + payload | `b` = 1 byte per element. three.js also accepts `c` |
| `S` | u32 len + bytes | not NUL-terminated, may contain `\0`, UTF-8. Seen: "INTÉRIEUR" in an SDK path |
| `R` | u32 len + bytes | raw, e.g. `FileId`, `Video.Content` |

**Reader quirks for these codes.**
- *Readers* should also accept the 1-byte scalars `B` and `Z`. SDK 2020.3 wrote `Z` for a `"UByte"` P value in a 7700
  file (ufbx accepts both); Assimp, OpenFBX and fbxcel reject them, so *writers* must never emit them.
- Assimp does not parse `b` arrays: it skips the rest of the record (`FBXBinaryTokenizer.cpp` L233-235). Avoid `b`
  arrays, or put them last.
- The array header is always 3× u32, even in 7500+.

For arrays, `Encoding=0` means the payload is `ArrayLength × elemsize` bytes, LE, and `CompressedLength` equals that byte
count. `Encoding=1` means the payload is a **zlib stream** (2-byte header, deflate, Adler-32). All 1,617 compressed arrays
in the four SDK files start `78 01`. The SDK's choice is size-based: the largest raw array seen was 939 bytes and the
smallest compressed one was 1,072 bytes. Element count is not the criterion. Readers accept either encoding.

**Types the SDK uses for `P` values in Properties70** (tally over 4 SDK files) **[V]**:
- `bool`, `Bool`, `enum`, `int`, `Integer` → `I`
- `double`, `Number`, `Visibility` → `D` (one value)
- `KTime`, `ULongLong` → `L`
- `KString`, `DateTime` → `S`
- `Color`, `ColorRGB`, `Vector`, `Vector3D`, `Lcl Translation`, `Lcl Rotation`, `Lcl Scaling` → `DDD`
- `ColorAndAlpha` → `DDDD`; `Float` → `F`; `Short` → `YYY`
- `Compound`, `object`, `Reference` → no value

A `P` record is `S name, S type, S label, S flags, values…`. The label is "" or e.g. "Integer", "Number", "Color",
"Vector", "Time", "Url", "XRefUrl". Flags are "" or "A" (animatable), "A+" (animated), "U" (user), "H" (hidden).

**Object names.** Each object record is `Type: L id, S "Name\x00\x01Class", S "SubClass"`, for example
`Model: L 74970864, S "mixamorig:Hips\x00\x01Model", S "LimbNode"`. ASCII FBX writes the same as `"Model::mixamorig:Hips"`;
note that the order is reversed. The name part may be empty (`"\x00\x01Geometry"`). three.js truncates at the first `\0`
[3js L4058-4070].

### 1.4 Footer [V]
Bytes after the top-level NULL record, as measured in SDK 2014–2020 files (`Samba`, `mixamo`, `morph-translation`,
`exampleWindow`):
```
[16] footer id (varies per file in SDK output; constant in Blender/Assimp output)
[p ] zero padding, p = 16 − (P % 16) ∈ 1..16 where P = offset after the footer id (16 when already aligned; see §1.5)
[4 ] 00 00 00 00
[4 ] u32 version (same as header)
[120] zeros
[16] F8 5A 8C 6A DE F5 D9 7E EC E9 0C E3 75 8F 29 0B
```
In all SDK files the version lands at an offset ≡ 4 (mod 16), and the file length ≡ 0 (mod 16). Assimp's
`WriteBinaryFooter` produces exactly this layout, with `pad = 16 - (pos % 16)`. Blender writes the 4 zero bytes *before*
the padding. Its files therefore have the version at offset ≡ 0 (mod 16) and a length ≡ 12 (mod 16). Blender-made FBX
files are a routine input to Unity, Unreal and Maya. **Inference:** the SDK does not require the exact SDK footer layout,
and the footer's alignment is not validated byte-exactly. No source says whether an entirely missing footer is fatal to
the SDK: **UNVERIFIED**, so always write one.

**Is a wrong footer fatal elsewhere?**
- **three.js:** the parse loop stops once fewer than 176 bytes remain (exact check: `((offset+176) & ~15) >= size` when
  size % 16 == 0, else `offset+176 >= size`) [3js L3539-3557]. **Without a footer, three.js silently skips any trailing
  top-level records that fall inside the last ~176 bytes.** My probe lost only the `Takes` record, which three.js ignores
  anyway.
- **Assimp:** it ignores the footer. A probe with no footer loaded fine.
- **ufbx and OpenFBX:** both ignore the footer (§1.5). **fbxcel** validates it strictly.

**FileId, CreationTime, footer id.** In SDK files all three vary per file, and the 16-byte FileId and footer id look like an
obfuscation of the timestamp: every footer id seen starts `FA BC A?`. Blender (all versions sampled, 3.6–4.4) and Assimp
both write one fixed, known-good triple:
- `FileId` (`R`, 16 bytes): `28 B3 2A EB B6 24 CC C2 BF C8 B0 2A A9 2B FC F1`
- `CreationTime` (`S`): `"1970-01-01 10:00:00:000"`
- footer id: `FA BC AB 09 D0 C8 D4 66 B1 76 FB 83 1C F7 26 7E`

Assimp's source names these `GENERIC_FILEID`, `GENERIC_CTIME`, `GENERIC_FOOTID` and `FOOT_MAGIC`. Its comment says the
values are copied from Blender because "we don't actually know how to generate these".

**Both FileId and footer id are deterministic functions of the `CreationTime` string.** The algorithm comes from
**ufbx_write** (MIT/Unlicense, `ufbx_write.c` ≈ L10844-10866), whose comment reads: *"Derived from actual file hashes …
based on work by @hamish-milne"*. I checked it against **all four SDK files plus the constant triple, and all five match
[V]**. In my own words:
```
order      = [0x11,0x12,0x05,0x06,0x0b,0x0c,0x08,0x09,0x14,0x15,0x00,0x01,0x02,0x03,0x0e,0x0f]
fileid_key = 18 b3 1a ea 86 24 fc c3 8e c9 80 23 97 25 c2 ff
footer_key = fa bc 9b 09 d0 c9 e4 67 b1 76 ca 82 1d ff 19 78
ct  = CreationTime bytes, "YYYY-MM-DD HH:MM:SS:mmm" (23 ASCII bytes)
s[i] = ct[order[i]]                                     (i = 0..15)
FileId:  m = s; for i in 1..16: m[i] ^= m[i-1];          then m[i] ^= fileid_key[i]
Footer:  m = s; for i in 2..16: m[i] ^= m[i-2];  m[i] ^= s[i] (all i);  for i in 1..16: m[i] ^= m[i-1];  m[i] ^= footer_key[i]
```
**Recommendation:** use the constant triple; it is the most widely deployed. If you want a real date, compute the two
hashes with this algorithm. Never pair a real date with the constant hashes. The header's `CreationTimeStamp` block may
hold the real date, as Blender and Assimp do. ufbx issue #229 (§1.5) suggests the SDK *does* check these hashes, but
there is no definitive proof: **UNVERIFIED**.

ufbx_write writes the footer layout of §1.4: *"Align to 16 bytes, always insert at least a single zero"*, then a
`uint32 footer[32]` holding 0, the version and zeros, then the fixed 16 bytes.

### 1.5 Readers' tolerance of footer and IDs (from source review)
Sources: ufbx 5955c5c, OpenFBX 4d4a45a, Assimp f6ee58e, fbxcel dba8425. The SDK-file checks below were run on 11 SDK
2019.2–2020.3 binaries from the ufbx test corpus.

**Do readers validate the footer?**
- **ufbx** ignores the footer. It stops at the top-level NULL record and relies on *"valid FBX files must end in a 13/25
  byte NULL record"* (`ufbx.c` ≈ L9043, L9262).
- **OpenFBX** ignores it.
- **Assimp** ignores it: *"fbx files have a mysterious extra footer which I don't know how to extract any information
  from, but at least it always starts with a 0"* (`FBXBinaryTokenizer.cpp` L309-315).
- **fbxcel** validates strictly (`fbx_footer.rs` L112-164). Its writer pads 0–15 bytes, which conflicts with the SDK's
  "16 when already aligned".

**The top-level NULL record is required.** ufbx treats an `EndOffset` and name length of 0 as the list terminator, and
OpenFBX errors with "Reading past the end" without it.

**ufbx caveat:** children of **top-level** records are read until a NULL record; `EndOffset` is ignored there
(`ufbx.c` ≈ L11308). So every top-level record with children must end with its NULL record.

**SDK footer with P % 16 == 0** (P = the offset right after the footer id): `max_unicode_7500_binary.fbx` has 16 padding
bytes plus 4 zero bytes, i.e. 20 zero bytes before the version. This confirms the "full 16" rule.

**Does the Autodesk SDK reject a bad footer, FileId or CreationTime?** The evidence points to **yes**, but it is not
conclusive.
- ufbx issue #229 (Oct 2025, https://github.com/ufbx/ufbx/issues/229): a hand-written *binary* FBX failed in the SDK with
  `Call to FbxImporter::Initialize() failed. None of the registered readers can process the file`, so it also failed in
  Unity and Blender's legacy importer. An ASCII conversion of the same content loaded.
- The ufbx author's diagnosis: *"The exported binary files have this weird footer. The easiest way to get readable files
  is to do what Blender does and use a fixed `FileId` and binary `CreationTime` values from a known good FBX file, as
  that's what the footer is based on."*
- Assimp PR #1819 (2018) said of footer *alignment*: *"The FBX SDK doesn't seem to care when importing"*.
- **Conclusion:** the SDK tolerates padding differences, as Blender's layout shows. Treat the
  `FileId` / `CreationTime` / footer-id triple as **mandatory and mutually consistent**: use the constant triple, or
  compute both hashes from the time (§1.4).

**IDs.** All of these readers take int64 IDs. three.js converts them to JS Numbers, so keep them below 2^53. Using
sequential IDs from 1,000,000 up, as Assimp does, or random 31-bit values, as Blender does, is safe.

### 1.6 Reader notes for untrusted vendor files (engineering guidance, not a format fact)
- **Versions.** Accept 7100–7700 and anything newer, with 64-bit record headers whenever version >= 7500. Mixamo now ships
  **7700** (SDK 2020.2) **[V]**. 7.7 adds `FBXHeaderExtension.FBXHeaderVersion 1004` and
  `OtherFlags { TCDefinition: I 127 }` **[V]**; keys in that file still use 46,186,158,000 ticks/s. Choose the tick
  rate as described in §7 ("Time base change").
- **Out of scope:** FBX 6.x (≤ 6100) uses string names instead of int64 IDs and `Properties60`; ASCII files start with
  `; FBX` or `FBXHeaderExtension:`.
- **Bounds checks.** Validate every `EndOffset`: it must not be less than the end of the property list and must not exceed
  the parent's end. Cap recursion depth.
- **Array decoding.** Require `CompressedLength` to stay inside the record. When `Encoding=0`, require
  `CompressedLength == ArrayLength × elemsize`. When `Encoding=1`, inflate with an output cap of
  `ArrayLength × elemsize` and reject a size mismatch; this guards against zlib bombs. Reject unknown encodings.
- **Geometry indices.** Validate `PolygonVertexIndex` (every index is ~idx-decoded and below the control point count; the
  last polygon is terminated), index arrays, `Indexes` in clusters and shapes, and layer array lengths against the mapping
  mode.
- **Strings.** Decode leniently: UTF-8 with replacement; split names at `\x00\x01`.
- **Writing 7400 or 7500.** 7400 is the most compatible choice. Blender writes 7400; Assimp writes 7500. Autodesk says 7.5
  is "only compatible with the 2016 or later product versions". 7400's u32 offsets limit a file to 4 GiB.

---------------------------------------------------------------------------------------------------------------------

## 2. Top-level structure

The SDK and Blender write the top-level records in this order **[V]**:
`FBXHeaderExtension, FileId, CreationTime, Creator, GlobalSettings, Documents, References, Definitions, Objects, Connections, Takes`
(the SDK 2020.3.x 3ds Max files omit `Takes` when there is no animation).

```
FBXHeaderExtension: {
  FBXHeaderVersion: I 1003          (1004 in the 7700 file)
  FBXVersion: I 7400|7500
  EncryptionType: I 0
  CreationTimeStamp: { Version: I 1000, Year, Month, Day, Hour, Minute, Second, Millisecond: I }
  Creator: S "..."
  SceneInfo: S "GlobalInfo\x00\x01SceneInfo", S "UserData" {          (SDK and Blender write it; Assimp does not)
     Type: S "UserData"; Version: I 100
     MetaData: { Version: I 100; Title/Subject/Author/Keywords/Revision/Comment: S "" }
     Properties70: { P "DocumentUrl","KString","Url","",…; P "SrcDocumentUrl",…; P "Original","Compound","",""; P "Original|ApplicationVendor","KString","","",…;
                     P "Original|ApplicationName",…; P "Original|ApplicationVersion",…; P "Original|DateTime_GMT","DateTime","","",…; P "Original|FileName",…;
                     P "LastSaved", "Compound"…; P "LastSaved|ApplicationVendor",… (same set) } }
}
FileId: R <16 bytes>          CreationTime: S "1970-01-01 10:00:00:000"          Creator: S "..."
GlobalSettings: { Version: I 1000; Properties70: { see list below } }
Documents: { Count: I 1; Document: L <id>, S "Scene" (or ""), S "Scene" { Properties70: { P "SourceObject","object","",""; P "ActiveAnimStackName","KString","","","<stack name>" }; RootNode: L 0 } }
References:            (empty record + NULL)
Definitions: { Version: I 100; Count: I <sum of all object counts incl. GlobalSettings>;
               ObjectType: S "GlobalSettings" { Count: I 1 }
               ObjectType: S "Model" { Count: I n; PropertyTemplate: S "FbxNode" { Properties70: { …defaults… } } } … }
Objects: { …one record per object… }
Connections: { C: S "OO", L child, L parent;   C: S "OP", L child, L parent, S "PropertyName"; … }
Takes: { Current: S "<stack>"; Take: S "<stack>" { FileName: S "<stack>.tak"; LocalTime: L start, L stop; ReferenceTime: L start, L stop } }
```

**GlobalSettings.Properties70.** The SDK (Mixamo, 2014.2.1) writes these values, types and order **[V]**:
`UpAxis int 1`, `UpAxisSign int 1`, `FrontAxis int 2`, `FrontAxisSign int 1`, `CoordAxis int 0`, `CoordAxisSign int 1`,
`OriginalUpAxis int 1`, `OriginalUpAxisSign int 1`, `UnitScaleFactor double 1.0`, `OriginalUnitScaleFactor double 1.0`,
`AmbientColor ColorRGB 0,0,0`, `DefaultCamera KString "Producer Perspective"`, `TimeMode enum 6`, `TimeProtocol enum 1`,
`SnapOnFrameMode enum 0`, `TimeSpanStart KTime`, `TimeSpanStop KTime`, `CustomFrameRate double -1`,
`TimeMarker Compound`, `CurrentTimeMarker int -1`. The labels are "Integer", "Number", "Color" and "Time", as in §1.3.

**Definitions.** Template names seen in SDK files **[V]**:
- Model → `FbxNode`
- NodeAttribute → `FbxSkeleton` or `FbxNull`
- Geometry → `FbxMesh`
- Material → `FbxSurfacePhong`, `FbxSurfaceLambert` or `FbxSurfaceMaterial`
- Texture → `FbxFileTexture`; Video → `FbxVideo`
- AnimationStack → `FbxAnimStack`, AnimationLayer → `FbxAnimLayer`, AnimationCurveNode → `FbxAnimCurveNode`
- Implementation → `FbxImplementation`, BindingTable → `FbxBindingTable`, CollectionExclusive → `FbxDisplayLayer`
- Deformer, Pose and AnimationCurve have only a `Count` and no template.

An object's properties override its type's template, and anything absent falls back to the template. Important template
defaults are `InheritType 0`, `RotationActive 0`, `DefaultAttributeIndex -1` and `RotationOrder 0`. **Write the values you
need explicitly on each object.**

**What can be omitted (measured).** Probes are in `probe/` and are loaded with Assimp 6.0 and three.js r183.
| file contains | Assimp 6.0 | three.js r183 |
|---|---|---|
| only `Objects` + `Connections` | **fails**: "FBX-DOM no FBXHeaderExtension dictionary found" | loads fully |
| `FBXHeaderExtension{FBXHeaderVersion,FBXVersion}` + Objects + Connections | loads | loads |
| everything except `Definitions` | loads | loads |
| everything, no footer | loads | loads (see §1.4 caveat) |
| everything, no `Pose` (mesh at identity) | loads | loads |

For Maya, 3ds Max, Unity, Unreal and Blender I could not run SDK-based tools here, and no source states which top-level
records the SDK requires: **UNVERIFIED**. Facts that bear on it:
- Blender's files carry every record and load in all SDK tools.
- Assimp's files omit `SceneInfo` and `Takes`.
- ufbx_write writes the full SDK-style set, including Takes.
- ufbx issue #229 shows the SDK rejects malformed binary files outright, with no partial load.

**Recommendation:** always write every top-level record above, `Definitions` with templates, and `SceneInfo`. This is
cheap and matches what both Blender and the SDK emit, so it is the best-tested input for every importer.

**Connections** **[V][3js]**:
- `OO` connects object to object; the child is listed first and the parent second. `OP` connects an object to a named
  property of the parent.
- `0` is the root. Every top-level Model needs `C "OO", model, 0`. three.js calls `connections.get(model.ID).parents` and
  throws if a Model has no connection at all.
- three.js treats a Geometry with no parent Model as unused. It ignores a Material with no connections.

---------------------------------------------------------------------------------------------------------------------

## 3. Geometry, models, transforms, skeleton nodes

### 3.1 Geometry "Mesh" [V]
SDK child order (Mixamo, 3ds Max):
```
Geometry: L id, S "Name\x00\x01Geometry", S "Mesh" {
  Properties70: { P "Color","ColorRGB","Color","",r,g,b }        (optional)
  Vertices: d[3*nCP]                   control points (x,y,z) in geometry space
  PolygonVertexIndex: i[nPV]           polygon corners. The last corner of each polygon is stored as ~idx = -(idx+1), e.g. a quad 0,1,2,3 -> 0,1,2,-4
  Edges: i[nEdges]                     optional (see below)
  GeometryVersion: I 124
  LayerElementNormal: I 0 { Version: I 101|102; Name: S ""; MappingInformationType: S; ReferenceInformationType: S;
                            Normals: d[3*k]; [NormalsIndex: i]; [NormalsW: d[k]] }      (Version 102 adds NormalsW, all 0 in SDK output)
  LayerElementBinormal / LayerElementTangent: same shape (Binormals/BinormalsW, Tangents/TangentsW)    (optional)
  LayerElementColor: I 0 { Version: I 101; Name; MIT; RIT; Colors: d[4*k] RGBA; [ColorIndex: i] }
  LayerElementUV: I n { Version: I 101; Name: S "map1"|"UVChannel_1"|...; MIT; RIT; UV: d[2*k]; [UVIndex: i] }
  LayerElementSmoothing: I 0 { Version: I 102; Name; MIT "ByPolygon" (groups) | "ByEdge" (hard edges); RIT "Direct"; Smoothing: i }  (optional)
  LayerElementMaterial: I 0 { Version: I 101; Name; MIT "AllSame"|"ByPolygon"; RIT "IndexToDirect"; Materials: i }
  Layer: I 0 { Version: I 100; LayerElement: { Type: S "LayerElementNormal"; TypedIndex: I 0 } … one per element of layer 0 }
  Layer: I 1 { Version: I 100; LayerElement: { Type: S "LayerElementUV"; TypedIndex: I 1 } }      (2nd UV set goes in layer 1)
}
```
- **Mapping strings** (exact spelling matters): `"ByPolygonVertex"` (one value per entry of PolygonVertexIndex),
  `"ByVertice"` (per control point; the SDK's `eByControlPoint`), `"ByPolygon"`, `"ByEdge"`, `"AllSame"`.
  `"NoMappingInformation"` also exists. three.js's `getData` only handles `ByPolygonVertex`, `ByPolygon`, `ByVertice` and
  `AllSame` [3js L4152-4182]. **Probe:** with `"ByControlPoint"`, three.js warns "unknown attribute mapping type" and emits
  garbage normals and UVs, and Assimp 6.0 drops the normals and UVs entirely. Use `"ByVertice"`.
- **Reference strings:** `"Direct"` (data indexed by the mapping index) and `"IndexToDirect"` (look up `*Index` first).
  `"Index"` is a legacy synonym. Index array names are `NormalsIndex` (three.js also accepts `NormalIndex`), `UVIndex`,
  `ColorIndex`, `BinormalsIndex` and `TangentsIndex`.
- **What each reader accepts** (agent's source review plus my probes):

  | reader | vertex-data mapping | references | materials | unknown string |
  |---|---|---|---|---|
  | three.js | ByPolygonVertex, ByPolygon, ByVertice, AllSame | IndexToDirect, else direct | AllSame / ByPolygon / … | warning + garbage **[V]** |
  | Assimp | **only** ByVertice, ByPolygonVertex | Direct, IndexToDirect | AllSame, or ByPolygon+IndexToDirect; **both type strings required** | channel dropped ("access type not implemented") **[V]** |
  | ufbx | ByPolygonVertex, ByVertice, ByVertex, ByPolygon, AllSame (ByEdge for smoothing, crease, visibility) | ignores the string; presence of `*Index` decides | | element dropped with a warning (ByControlPoint) |
  | OpenFBX | ByPolygonVertex, ByPolygon, ByVertice, ByVertex | Direct, IndexToDirect only | ByPolygon+IndexToDirect or AllSame | **whole load fails** |

  **Safe set:** `ByPolygonVertex` or `ByVertice` for vertex data; `AllSame`, or `ByPolygon`+`IndexToDirect`, for materials;
  `Direct` or `IndexToDirect` only.
- **The `Layer` record is required by Assimp.** Assimp locates layer elements *only* through
  `Layer → LayerElement{Type, TypedIndex}`; otherwise it reports "failed to resolve vertex layer element"
  (`FBXMeshGeometry.cpp` ≈ L278-307). ufbx and OpenFBX read `LayerElement*` directly.
- **Assimp drops connected materials when `LayerElementMaterial` is absent** ("no material assigned to mesh, setting
  default material"). Always write one.
- **Recommended for glTF→FBX:**
  - Positions: deduplicate to control points if you want welded topology. Writing glTF's split vertices 1:1 as control
    points, as Assimp does, is also valid.
  - Normals: `ByPolygonVertex` + `Direct`, as the SDK does.
  - UVs: `ByPolygonVertex` + `IndexToDirect`, with the UV array deduplicated.
  - Colors: `ByPolygonVertex` + `IndexToDirect` or `Direct`.
  - Material: `AllSame` with `[0]` for one material, otherwise `ByPolygon` + `IndexToDirect` with one index per polygon
    **[V]** (3ds Max file).
- **Material index** = position in the list of `Material` objects connected `OO` to the **Model** (not to the Geometry),
  in connection order. The SDK connects materials Model-side **[V]**. three.js builds its material array from the Model's
  children in connection order [3js L1279-1319].
- **Edges:** written by the SDK, Blender and ufbx_write; not by Assimp. three.js, Assimp and OpenFBX ignore it. For
  ufbx it is optional: each entry indexes into `PolygonVertexIndex` and denotes the edge from that corner to the next
  corner of its polygon. **UNVERIFIED** whether any SDK-based importer requires it; writing it is cheap.
- **Layers:** three.js ignores `Layer` records and reads `LayerElement*` children directly. It uses the first Normal,
  Color and Material elements and all UV elements [3js L1778-1820]. Write `Layer` records anyway, because Assimp and SDK
  tools use them. Put UV set *n* in `Layer n` **[V]**.
- **Smoothing:** ufbx reads it as per-edge or per-face booleans, so smoothing-group bitmasks collapse. Assimp ignores it.
  When normals are written, smoothing is unnecessary.
- **W arrays:** `NormalsW`, `TangentsW` and `BinormalsW` hold one double per element. ufbx loads them only on request,
  and Assimp's exporter sticks to LayerElementNormal Version 101 without them (*"not sure what it is, so stick with
  version 101"*).
- **Polygons:** n-gons are allowed. three.js triangulates with earcut, Assimp triangulates. Emitting triangles from glTF is
  simplest.
- **UV V-axis:** FBX uses the OpenGL/OBJ convention, with V = 0 at the bottom of the image. **[V]** Assimp turned glTF UV
  (0,1) into FBX (0,0). three.js loads FBX textures with `flipY = true`, but glTF textures with `flipY = false`.
  ufbx, Assimp's FBX importer and OpenFBX never flip V. Assimp flips only in its glTF importer
  (`glTF2Importer.cpp`: `values[i].y = 1 - values[i].y; // Flip Y coords`). So glTF→FBX is `v_fbx = 1 - v_gltf`, and
  FBX→glTF is the same.
- **Vertex colors:** three.js treats FBX `Colors` as **sRGB** and converts them to linear. It drops alpha [3js L2377-2405].
  glTF `COLOR_0` is linear, so convert linear→sRGB when writing. Blender agrees: both the FBX exporter and the importer
  default to `colors_type='SRGB'`, documented as *"Export colors in sRGB color space"* and *"Expect file colors in sRGB
  color space"* (https://docs.blender.org/api/current/bpy.ops.export_scene.html,
  https://docs.blender.org/api/current/bpy.ops.import_scene.html). **UNVERIFIED** for Maya.

### 3.2 Model node [V]
```
Model: L id, S "Name\x00\x01Model", S "Mesh"|"Null"|"LimbNode"|"Root"|"Camera"|"Light" {
  Version: I 232
  Properties70: { P "Lcl Translation","Lcl Translation","","A",x,y,z; P "Lcl Rotation","Lcl Rotation","","A",rx,ry,rz (degrees);
                  P "Lcl Scaling","Lcl Scaling","","A",sx,sy,sz; P "RotationOrder","enum","","",0; P "InheritType","enum","","",1;
                  P "RotationActive","bool","","",1; P "DefaultAttributeIndex","int","Integer","",0; …optional pivots… }
  Shading: C 1 (SDK writes 'Y' or 'T')
  Culling: S "CullingOff"
}
```
Use flag "A+" instead of "A" on animated properties; the SDK writes "A+" on animated properties.

**Model Properties70 names, types and defaults** (from the SDK `FbxNode` template, which matches Assimp's) **[V]**:
| property | type | default |
|---|---|---|
| `QuaternionInterpolate` | enum | 0 |
| `RotationOffset`, `RotationPivot`, `ScalingOffset`, `ScalingPivot` | Vector3D | 0 |
| `RotationOrder` | enum | 0 |
| `PreRotation`, `PostRotation` | Vector3D | 0 |
| `RotationActive` | bool | 0 |
| `InheritType` | enum | **0** |
| `GeometricTranslation`, `GeometricRotation` | Vector3D | 0 |
| `GeometricScaling` | Vector3D | 1 |
| `DefaultAttributeIndex` | int | **-1** |
| `Lcl Translation`, `Lcl Rotation` | Lcl … | 0 |
| `Lcl Scaling` | Lcl Scaling | 1 |
| `Visibility` | Visibility | 1 |
| `Visibility Inheritance` | Visibility Inheritance | 1 |

The template also holds limits (`TranslationMin`, `RotationMax`, …) and damping fields. They are irrelevant here.

**Local transform (SDK convention)** [SDK-doc; 3js `generateTransform` L4190-4305]:
```
L = T · Roff · Rp · Rpre · R · Rpost⁻¹ · Rp⁻¹ · Soff · Sp · S · Sp⁻¹          (column vectors, applied right-to-left)
World(node) = World(parent) ⊗ L     (⊗ depends on InheritType, below)
World(geometry) = World(node) · Gt · Gr · Gs      (Geometric* : NOT inherited by children)
```
**Primary sources, and the documentation inconsistency:**
- The developer guide page "Computing transformation matrix" gives
  *"WorldTransform = ParentWorldTransform * T * Roff * Rp * Rpre * R * Rpost-1 * Rp-1 * Soff * Sp * S * Sp-1"* and
  defines Rpost-1 as *"Inverse of the post-rotation"*
  (https://help.autodesk.com/cloudhelp/2019/ENU/FBX-Developer-Help/nodes_and_scene_graph/fbx_nodes/computing_transformation_matrix.html).
  It also states *"for the default rotation order of XYZ, R = Rz * Ry * Rx"*.
- The FbxNode class reference ("Pivot Management") writes `… * R * Rpost * Rp-1 …`, **without** the inverse, and so does
  the comment in the SDK's Transformations sample.
- three.js, Assimp, Blender, ufbx and OpenFBX all apply **Rpost⁻¹**.
  - three.js: L4235.
  - ufbx: *"NOTE: Rpost is inverted (!) after converting from PostRotation Euler angles"* (`ufbx.c` ≈ L22855).
  - ufbx docs: *"PostRotation: Inverse Euler rotation applied after "Lcl Rotation", always XYZ order"*.
  - OpenFBX: `… * r * r_post_inv * …` (`ofbx.cpp` L4231).

  Use the inverse in readers. A glTF writer never emits PostRotation, so the question does not arise there.
- Geometric transforms: *"The geometric transformation (Gt * Gr * Gs) is applied only to the node attribute and after the
  node transformations. This transformation is not inherited across the node hierarchy."* (FbxNode class reference.)
- `FbxAMatrix` is column-major, with *"the last row of the matrix represents the translation part"*. This matches the
  16-double arrays in the file: translation sits at indices 12–14.
- T, Roff, Rp, Soff and Sp are translations by `Lcl Translation`, `RotationOffset`, `RotationPivot`, `ScalingOffset` and
  `ScalingPivot`. S is the scale by `Lcl Scaling`.
- R is `Lcl Rotation`, in degrees, in `RotationOrder`. Rpre and Rpost are `PreRotation` and `PostRotation` in degrees,
  **always XYZ order** regardless of `RotationOrder`. three.js applies exactly this, with the comment "For Maya models
  using 'Joint Orient', Euler order only applies to rotation, not pre/post-rotations" [3js L4211-4237].
- **PostRotation is applied inverted.**
- **RotationOrder** values: 0 `eEulerXYZ`, 1 `eEulerXZY`, 2 `eEulerYZX`, 3 `eEulerYXZ`, 4 `eEulerZXY`, 5 `eEulerZYX`,
  6 `eSphericXYZ` (unsupported by three.js, Assimp and ufbx). FBX orders are **extrinsic**: XYZ means rotate about X
  first, then Y, then Z. ufbx.h puts it this way: *"The order in the name refers to the order of axes *applied*, not the
  multiplication order: eg. `UFBX_ROTATION_ORDER_XYZ` is `Z*Y*X`"*.
  The matrix is `R = Rz·Ry·Rx` for XYZ. Likewise XZY → `Ry·Rz·Rx`, YZX → `Rx·Rz·Ry`, YXZ → `Rz·Rx·Ry`, ZXY → `Ry·Rx·Rz`,
  ZYX → `Rx·Ry·Rz`. three.js maps FBX 0..5 to its intrinsic `'ZYX','YZX','XZY','ZXY','YXZ','XYZ'` [3js L4309-4332].
- **InheritType** (three.js implements the SDK sample) [3js L4270-4286]. The values come from
  `enum EInheritType {eInheritRrSs, eInheritRSrs, eInheritRrs}` in `fbxtransforms.h`. The class reference explains them:
  *"eInheritRrSs : Scaling of parent is applied in the child world after the local child rotation. eInheritRSrs : Scaling
  of parent is applied in the parent world. eInheritRrs : Scaling of parent does not affect the scaling of children."*
  - **0 eInheritRrSs**: GlobalRS = Rparent·Rlocal·Sparent·Slocal.
  - **1 eInheritRSrs**: GlobalRS = Rparent·Sparent·Rlocal·Slocal, which is ordinary matrix composition and glTF semantics.
  - **2 eInheritRrs**: parent *local* scale is not inherited (Maya "segment scale compensate", seen on Mixamo joints).

  **Write `InheritType = 1` on every Model** for glTF content, because the template default is 0. Mixamo writes 1 on the
  root and 2 on child joints; Assimp and Blender write 1. **[V]**
- **RotationActive** [SDK-doc, `FbxNode::SetRotationActive`]: *"When this flag is set to false, the RotationOrder, the
  Pre/Post rotation values and the rotation limits should be ignored."* The SDK 2015 notes add that axis conversion *"now
  ignores the pre/post rotation and rotation order values on a node if the RotationActive property is set to false"*.
  The template default is **0**. Maya, Mixamo and Assimp write 1; Blender omits it.
  - three.js, Assimp and OpenFBX ignore the flag and always apply RotationOrder and pre/post rotation.
  - **ufbx follows the SDK doc.** It computes `use_rotation_space = RotationActive && !RotationSpaceForLimitOnly`, and when
    that is false it ignores PreRotation, PostRotation **and RotationOrder**, evaluating Lcl Rotation as XYZ
    (`ufbx.c` ≈ L22875-22882, L22964-22966). That affects Godot 4.3+ and Blender 4.5+.
  - **Writer:** always write `RotationActive = 1` whenever RotationOrder ≠ 0 or any pre/post rotation is non-zero. The
    simplest rule is to write it on every Model, as Assimp does.
  - **Reader:** follow ufbx, which matches the SDK doc.
- **Readers' InheritType support:** ufbx maps 0 → component-wise scale, 2 → ignore parent scale, else normal. Assimp and
  OpenFBX ignore InheritType. Writing 1 keeps everyone in agreement.
- **Geometric transforms: readers disagree.**
  - ufbx always uses XYZ for GeometricRotation.
  - Assimp applies the node's RotationOrder to it.
  - OpenFBX composes in a different order.

  **Do not write Geometric\*** from a glTF source; bake into vertices instead.
- **Assimp pivots.** By default (`AI_CONFIG_IMPORT_FBX_PRESERVE_PIVOTS` = true) Assimp turns any non-default
  pivot/offset/pre-post/geometric component into helper nodes named `<Name>_$AssimpFbx$_<Component>`. Its exporter folds
  them back. FBX→glTF converters built on Assimp should expect these nodes.
- **For glTF→FBX:** put the node TRS in `Lcl Translation/Rotation/Scaling` with RotationOrder 0. The quaternion becomes
  Euler XYZ degrees with `R = Rz·Ry·Rx`; Assimp wrote glTF `q = (0, sin45°, 0, cos45°)` as `Lcl Rotation = (0, 90, 0)`
  **[V]**. Leave pivots, offsets and pre/post rotation at zero, and Geometric* at identity. **For FBX→glTF:** bake the
  full formula into a matrix. Fold Geometric* into the mesh vertices; three.js does this [3js L1659-1676]. Pre/post
  rotations become part of node rotation, and animation must be baked through the same formula.

### 3.3 NodeAttribute and bones [V]
- Skeleton joint: the Model has class `"LimbNode"`, plus `NodeAttribute: L id, S "Name\x00\x01NodeAttribute", S "LimbNode" { TypeFlags: S "Skeleton" }`,
  connected `C "OO", attrId, modelId`. The `FbxSkeleton` template has `Size double 100` and `LimbLength double 1`, with
  flag "H". Mixamo uses `LimbNode` for every joint, including Hips **[V]**. A skeleton "Root" type (Model subclass
  `"Root"`) also exists. three.js turns Model classes `LimbNode` and `Root` into `THREE.Bone` [3js L989-992].
- Null/group: the Model has class `"Null"` (optionally with `NodeAttribute … "Null" { TypeFlags: S "Null" }`). Assimp
  writes no attribute for Null models.
- `DefaultAttributeIndex = 0` when the node has an attribute or geometry.
- three.js also makes a Bone of any Model that is linked to a Cluster [3js L1016-1061].

---------------------------------------------------------------------------------------------------------------------

## 4. Materials and textures

### 4.1 Material object [V]
```
Material: L id, S "Name\x00\x01Material", S "" {
  Version: I 102
  ShadingModel: S "phong"|"lambert"    (3ds Max Physical: "unknown"; Blender writes "Phong")
  MultiLayer: I 0
  Properties70: { … }
}
```
**`FbxSurfacePhong` template (SDK defaults)** **[V]**:
| property | type | default | property | type | default |
|---|---|---|---|---|---|
| `ShadingModel` | KString | "Phong" | `TransparentColor` | Color | 0,0,0 |
| `MultiLayer` | bool | 0 | `TransparencyFactor` | Number | 0 |
| `EmissiveColor` | Color | 0,0,0 | `DisplacementColor` | ColorRGB | 0,0,0 |
| `EmissiveFactor` | Number | 1 | `DisplacementFactor` | double | 1 |
| `AmbientColor` | Color | 0.2 | `VectorDisplacementColor` | ColorRGB | 0,0,0 |
| `AmbientFactor` | Number | 1 | `VectorDisplacementFactor` | double | 1 |
| `DiffuseColor` | Color | 0.8 | `SpecularColor` | Color | 0.2 |
| `DiffuseFactor` | Number | 1 | `SpecularFactor` | Number | 1 |
| `Bump` | Vector3D | 0 | `ShininessExponent` | Number | 20 |
| `NormalMap` | Vector3D | 0 | `ReflectionColor` | Color | 0,0,0 |
| `BumpFactor` | double | 1 | `ReflectionFactor` | Number | 1 |

The SDK (Maya/Mixamo) also writes **legacy FBX6-style computed** properties **[V]**:
`P "Emissive","Vector","","A+U"`, `"Ambient"`, `"Diffuse"`, `"Specular"`, `"Shininess","Number"`, `"Opacity","Number"`,
`"Reflectivity","Number"`. In the Mixamo file, `Diffuse = DiffuseColor × DiffuseFactor` (0.837 × 0.8 = 0.6696).

**three.js reads the legacy names first:**
- `Diffuse`, else `DiffuseColor`, but only if its type is "Color" or "ColorRGB".
- `Emissive`, else `EmissiveColor`; `EmissiveFactor` sets the intensity.
- `Specular`, else `SpecularColor`; `Shininess` (**not** `ShininessExponent`).
- `ReflectionFactor` becomes reflectivity.
- All colors are treated as **sRGB** [3js L560-647].

So when you write both legacy and new properties they must be consistent, and the legacy color must already include its
factor. three.js also applies `EmissiveFactor` *on top of* the legacy `Emissive`.

The simplest consistent choice is to write `DiffuseFactor = EmissiveFactor = SpecularFactor = 1` and premultiplied colours
in both the new and the legacy properties.

**Transparency.** The SDK's semantics are transparency = `TransparentColor × TransparencyFactor`. Mixamo writes
`TransparencyFactor 1.0` with `TransparentColor 0,0,0` and `Opacity 1`, which is opaque. three.js uses this logic
[3js L604-624]:
1. Compute `1 − TransparencyFactor`.
2. If that is exactly 0 or 1, use `Opacity` if present.
3. Otherwise fall back to `1 − TransparentColor.r`.

Writing alpha α as `TransparentColor = 1,1,1`, `TransparencyFactor = 1 − α` and `Opacity = α` is consistent for the SDK
semantics and for three.js. Assimp writes `Opacity` only (0.5 for my glTF alpha 0.5).

Primary sources [SDK-doc] use two readings:
- `TransparencyFactor` is *"(0 = opaque, 1 = transparent)"*. The SDK's DisplayMaterial sample prints
  `Opacity = 1.0 - TransparencyFactor`.
- The `FbxSurfaceMaterialUtils` doc example shows `Opacity 0.632` for `TransparentColor 0.368` with factor 1, i.e.
  Opacity = 1 − TransparentColor × TransparencyFactor.

With `TransparentColor = 1,1,1`, both readings give Opacity = α. Other importers:
- **Unity URP:** `Opacity` first, else 1 − TransparencyFactor, else from TransparentColor.
- **Assimp:** `Opacity`, else 1 − TransparencyFactor·avg(TransparentColor). Its comment: *"both the FBX SDK and Blender
  always write an additional legacy 'Opacity' field"*.
- **Blender:** writes `TransparencyFactor = 1 − α` (ufbx test data).

All of them are satisfied by the encoding above.

### 4.2 PBR carriage conventions
FBX has **no standard PBR material**. The only standard classes are FbxSurfaceLambert and FbxSurfacePhong. Blender's FBX
maintainer wrote in 2019: *"FBX only supports basic old-fashioned shading currently, so indeed the translation from the
Principled shader is rather lose… this is a known limitation"* (https://projects.blender.org/blender/blender/issues/69286).
Four conventions exist in practice. Sources are the agent's reading of real exporter files in ufbx's test data
(https://github.com/ufbx/ufbx/tree/master/data, MIT) plus my own dump of an SDK file.

1. **Blender's deterministic Phong encoding.** Blender's exporter writes Principled BSDF as legacy Phong:
   - `DiffuseColor` = base colour
   - `ShininessExponent` (and `Shininess`) = **(10·(1−roughness))²**
   - `ReflectionFactor` = metallic
   - `TransparencyFactor` = 1−α
   - `EmissiveColor` / `EmissiveFactor` = emission
   - normal map on `NormalMap`

   Three readers decode it:
   - **ufbx**: shader type `UFBX_SHADER_BLENDER_PHONG`, roughness = 1−√x/10, used when the exporter is Blender with FBX IO
     version ≥ 4.12.0. Its test confirms roughness 0.123 ↔ ShininessExponent 76.913.
   - **Assimp**: *"Match Blender behavior to extract roughness when only shininess is present"*; it maps a
     ReflectionFactor texture to METALNESS.
   - **Unity URP**: smoothness = √(Shininess·0.01) = 1−roughness.
2. **Maya Stingray PBS** (ShaderFX).
   - The material has `ShadingModel "unknown"` and `"Maya|TypeId" int 1166017`.
   - Properties: `Maya|base_color` (Vector3D), `Maya|metallic`, `Maya|roughness`, `Maya|emissive` (Vector3D),
     `Maya|emissive_intensity`, and float `Maya|use_{color,metallic,roughness,normal,emissive,ao}_map` flags.
   - Textures connect `OP` to `Maya|TEX_{color,metallic,roughness,normal,emissive,ao}_map`. There are also
     `Maya|uv_offset`/`uv_scale` and the IBL slots `Maya|TEX_global_diffuse_cube` etc.
   - Each material has an `Implementation` object (`ShaderLanguage "SFX"`, `RenderAPI "SFX_PBS_SHADER"`, plus a
     `ShaderGraph` blob) and a `BindingTable` whose `Entry` rows map `"Maya|base_color"` → `"base_color"`.
3. **Arnold `aiStandardSurface`** (`Maya|TypeId 1138001`; Implementation `AiOSL` / `ARNOLD_SHADER_ID`) and **Autodesk
   Standard Surface** (TypeId 1398031443; `StandardSSL` / `OSL`) share the property names `Maya|baseColor`, `Maya|base`,
   `Maya|metalness`, `Maya|specularRoughness`, `Maya|emission`, `Maya|emissionColor`, `Maya|opacity` and
   `Maya|normalCamera`.
4. **3ds Max.**
   - **Physical Material** (verified in an SDK 2020.3.4 file, §4.2 below), ClassIDs `1030429932 / -559038463`:
     `3dsMax|Parameters|base_color`, `metalness`, `roughness`, `roughness_inv`, `…_map` / `…_map_on`.
   - **glTF Material**, ClassIDs `943849874 / 1174294043`: `3dsMax|main|baseColor`, `baseColorMap`, `metalness`,
     `roughness`, `normalMap`, `ambientOcclusionMap`, `emissionColor`, `alphaMode`, `alphaCutoff`.
   - **PBR Metal/Rough** (prefix `3dsMax|main|`) and **OpenPBR** (`3dsMax|Parameters|base_metalness`, …).
   - Max also fills the classic Phong props with approximations.

**Importers:**
- **Unity**, two import modes (https://docs.unity3d.com/Manual/FBXImporter-Materials.html):
  - "Standard (Legacy)".
  - "Import via MaterialDescription", whose built-in remappers handle *"FBX Standard Material, Arnold Standard, Autodesk
    Interactive, and 3ds Physical Material"*.
  - Per the URP postprocessors:
    - Stingray is recognised **only if the Implementation has `RenderAPI == "SFX_PBS_SHADER"`**.
    - Arnold by Maya TypeId 1138001; 3ds Max materials by ClassID.
    - The plain FBX path reads Opacity / TransparencyFactor, DiffuseColor×DiffuseFactor, Bump or NormalMap,
      EmissiveColor×EmissiveFactor and Shininess. It reads **no metallic or AO**.
- **Unreal:** the legacy FBX pipeline says *"Only basic materials are supported for import with the FBX pipeline"*
  (https://dev.epicgames.com/documentation/en-us/unreal-engine/fbx-material-pipeline-in-unreal-engine). Interchange
  (production-ready since 5.5) instantiates Phong-style master materials. No evidence was found that Unreal reads
  Stingray, Physical or metallic/roughness from FBX: **UNVERIFIED** either way.
- **Blender:** the legacy importer converts Phong to Principled. Blender 4.5's new C++ importer *"based on ufbx"* imports
  *"more shader parameters"* (https://developer.blender.org/docs/release_notes/4.5/pipeline_assets_io/).
- **ufbx** recognises Stingray, Arnold and Standard Surface only via the Implementation RenderAPI, and 3ds Max
  materials via ClassIDs.

**What three.js and Assimp map back:**
- **three.js** creates only `MeshPhongMaterial` or `MeshLambertMaterial`. It maps texture connections by OP property name
  [3js L650-729]:
  - `DiffuseColor` or `Maya|TEX_color_map` → map
  - `NormalMap` or `Maya|TEX_normal_map` → normalMap
  - `Bump` → bumpMap
  - `EmissiveColor` → emissiveMap
  - `SpecularColor` → specularMap
  - `TransparentColor` or `TransparencyFactor` → alphaMap
  - `ReflectionColor` → envMap
  - `DisplacementColor` → displacementMap
  - `Maya|TEX_ao_map` → aoMap
  - `ShininessExponent`, `SpecularFactor`, `AmbientColor` and `VectorDisplacementColor` are skipped with a warning.

  There is **no metallic/roughness** support in three.js FBXLoader. Its opacity fallback differs by version: r183 uses
  `1 − TransparentColor.r` when neither TransparencyFactor nor Opacity decides; three.js dev (2026) uses 1. Writing an
  explicit `Opacity` avoids both.
- **Assimp** (`code/AssetLib/FBX/FBXConverter.cpp`, BSD-3):
  - Legacy texture slots: `DiffuseColor`→DIFFUSE, `NormalMap`→NORMALS, `Bump`→HEIGHT, `EmissiveColor`→EMISSIVE,
    `TransparentColor`/`TransparencyFactor`→OPACITY, `ShininessExponent`→SHININESS, `ReflectionFactor`→**METALNESS**.
  - Stingray textures: `Maya|TEX_color_map`→BASE_COLOR, `TEX_normal_map`→NORMAL_CAMERA, `TEX_emissive_map`→EMISSION_COLOR,
    `TEX_metallic_map`→METALNESS, `TEX_roughness_map`→DIFFUSE_ROUGHNESS, `TEX_ao_map`→AMBIENT_OCCLUSION.
  - Stingray scalars: `Maya|base_color`, `metallic`, `roughness`, `emissive`, `emissive_intensity` and the `use_*_map`
    flags. These are read **without** needing an Implementation object.
  - 3ds Max: `3dsMax|Parameters|{base_color,bump,metalness,roughness}_map` and `3dsMax|main|*_map`.
  - Opacity: the `Opacity` property if present, else 1 − TransparencyFactor·avg(TransparentColor).
  - Roughness from ShininessExponent via the Blender formula.
  - Every other property is exposed as `$raw.<name>`.

  - Source review of Assimp f6ee58e (`FBXConverter.cpp` ≈ L2131-2420): **PBR scalar values come only from the Stingray
    names** `Maya|base_color`, `metallic`, `roughness` and `emissive_intensity`, plus the `use_*_map` flags.
  - Arnold texture slots are also mapped: `Maya|baseColor`, `normalCamera`, `emissionColor`, `metalness`,
    `specularRoughness`, ….
  - Only ShadingModel `phong` and `lambert` get templates.
  - Without a `LayerElementMaterial`, the connected materials are ignored.
- **ufbx** picks the shader type in this order: the `Implementation.RenderAPI` (`ARNOLD_SHADER_ID`, `OSL`,
  `SFX_PBS_SHADER`), then `ShadingModel` lambert or phong, then the 3ds Max ClassIDs. It has complete PBR mapping tables
  for each type (`ufbx.c` ≈ L19488-19840). Blender's Phong encoding is decoded only with `use_blender_pbr_material`.

**Recommendation for glTF→FBX:**
1. **Always write a complete Phong material using Blender's encoding.** It is the one form every importer reads.
   - `ShadingModel "phong"`, `DiffuseColor` = base colour RGB, `DiffuseFactor` 1.
   - `Shininess` = `ShininessExponent` = (10(1−r))².
   - `ReflectionFactor` = metallic.
   - `EmissiveColor` = emissiveFactor, `EmissiveFactor` = 1, or the KHR_materials_emissive_strength value.
   - Normal texture on `NormalMap`, not `Bump`, with `BumpFactor` 1.
   - Transparency per §4.1: `TransparentColor 1,1,1`, `TransparencyFactor 1−α`, `Opacity α`.
   - Legacy duplicates `Diffuse`, `Emissive` and `Specular`, consistent with the new names.
   - **Never** connect the RGBA base-colour texture to `TransparentColor`, because three.js would use it as an alphaMap.
2. **Additionally** write the Stingray-style `Maya|…` scalars and `Maya|TEX_*_map` texture connections on the same
   material. Assimp then gets real PBR, and three.js gets an AO map, the only AO slot any importer reads.
   - Adding the `Implementation` + `BindingTable` objects (`RenderAPI "SFX_PBS_SHADER"`) would switch Unity to
     "Autodesk Interactive" and ufbx/Blender 4.5 to PBR. But Maya expects a ShaderGraph blob, and mixing it with Phong is
     untested: treat it as **UNVERIFIED, opt-in, test in Maya and Unity first**.
3. **Split glTF's packed occlusion/roughness/metallic texture** into separate single-channel images. FBX texture slots
   have no channel selector.
4. Do not spoof 3ds Max ClassIDs. Ship the GLB alongside the FBX when lossless PBR matters.
5. **Colour space conflict.** three.js treats FBX colours as **sRGB** [3js]. Unity URP's MaterialDescription path treats
   them as **linear** (it applies `.gamma` in Linear projects). glTF factors are linear. Write linear values, as Blender
   does, and accept the small shift in three.js. This is an inference; **UNVERIFIED** for Maya.

Full Stingray property dumps and the Unity postprocessor key lists:
https://github.com/Unity-Technologies/Graphics/tree/master/Packages/com.unity.render-pipelines.universal/Editor/AssetPostProcessors.

**Observed in an SDK file: 3ds Max Physical Material** (`morph-translation.fbx`, SDK 2020.3.4) **[V]**:
- `ShadingModel "unknown"`, plus the classic props as `ColorRGB` and `double`.
- Identity props: `"3dsMax","Compound"`, `"3dsMax|ClassIDa" int 1030429932`, `"3dsMax|ClassIDb" int -559038463`,
  `"3dsMax|SuperClassID" int 3072`, `"3dsMax|ORIGINAL_MTL" KString "PHYSICAL_MTL"`.
- About 120 `"3dsMax|Parameters|…"` properties typed `Float`/`ColorAndAlpha`/`Bool`/`Integer`/`Reference`, e.g.
  `base_weight`, `base_color` (RGBA), `roughness`, `roughness_inv`, `metalness`, `reflectivity`, `emission`, `emit_color`,
  `transparency`, `coating`, and per-map `…_map` (Reference), `…_map_on` (Bool) and `displacement_map_amt`.
- Textures are connected **twice**: `C "OP", tex, mat, "3dsMax|Parameters|base_color_map"` **and** `C "OP", tex, mat, "DiffuseColor"`.
- Each material also gets an `Implementation` object (`ShaderLanguage "StandardSSL"`, `RenderAPI "OSL"`) and a
  `BindingTable` object, both connected `OO` → Material.

### 4.3 Texture and Video objects [V]
SDK form (3ds Max 2020.3.4; Maya is the same):
```
Video: L id, S "Name\x00\x01Video", S "Clip" {
  Type: S "Clip"
  Properties70: { P "Path","KString","XRefUrl","", "<abs path>"; P "RelPath","KString","XRefUrl","", "<rel path>" }
  UseMipMap: I 0
  Filename: S "<abs path>"          RelativeFilename: S "<rel path>"
  Content: R <entire image file bytes>          (embedded media; omit it when not embedded. The SDK writes a zero-property `Content` record there, with no NULL)
}
Texture: L id, S "Name\x00\x01Texture", S "" {
  Type: S "TextureVideoClip"
  Version: I 202
  TextureName: S "Name\x00\x01Texture"
  Properties70: { P "UVSet","KString","","","<uv set name>"; P "UseMaterial","bool","","",1; [Translation/Rotation/Scaling "Vector","","A"; WrapModeU/V enum 0=repeat 1=clamp; …] }
  Media: S "Name\x00\x01Video"
  FileName: S "<abs path>"          RelativeFilename: S "<rel path>"
  ModelUVTranslation: D 0, D 0      ModelUVScaling: D 1, D 1
  Texture_Alpha_Source: S "None" | "Alpha_Black" | …
  Cropping: I 0, I 0, I 0, I 0
}
C "OO", videoId, textureId          C "OP", textureId, materialId, "DiffuseColor"   (property name = material slot)
```
- **`FbxFileTexture` template (Assimp/SDK) defaults:** `TextureTypeUse enum 0`, `Texture alpha Number 1`,
  `CurrentMappingType enum 0`, `WrapModeU/V enum 0`, `UVSwap bool 0`, `PremultiplyAlpha bool 1`,
  `Translation/Rotation Vector 0`, `Scaling Vector 1`, `TextureRotationPivot/TextureScalingPivot Vector3D 0`,
  `CurrentTextureBlendMode enum 1`, `UVSet KString "default"`, `UseMaterial bool 0`, `UseMipMap bool 0`.
- **Enums [SDK-doc]** (`fbxtexture.h`, FBX SDK 2020 reference):
  - `WrapModeU/V`: 0 eRepeat (default), 1 eClamp. glTF MIRRORED_REPEAT has no FBX equivalent.
  - Texture alpha source (`Texture_Alpha_Source` string): eNone=0 "None", eRGBIntensity=1, eBlack=2 "Alpha_Black".
    The string "RGB_Intensity" is UNVERIFIED.
  - `CurrentTextureBlendMode`: eTranslucent=0, eAdditive=1 (default), eModulate=2, eModulate2=3, eOver=4.
  - `CurrentMappingType` uses EUnifiedMappingType: eUMT_UV=0 (default) … eUMT_NO_MAPPING=10.
  - UV transform: *"A displacement of one unit is equal to the texture's width after applying U scaling"*.
    Rotation is in degrees.
- **Slot names** (the OP property name is the material property being driven):
  - classic: `DiffuseColor`, `DiffuseFactor`, `EmissiveColor`, `EmissiveFactor`, `AmbientColor`, `SpecularColor`,
    `SpecularFactor`, `ShininessExponent`, `NormalMap`, `Bump`, `TransparentColor`, `TransparencyFactor`,
    `ReflectionColor`, `ReflectionFactor`, `DisplacementColor`, `VectorDisplacementColor`
  - vendor-prefixed: `Maya|…` and `3dsMax|Parameters|…_map`
- **Embedded textures.** `Video.Content` holds the raw PNG/JPEG file bytes **[V]**. Both Blender and the SDK write them,
  and Assimp embeds them when the glTF source had embedded images.
  - **three.js** builds a blob from `Content` when it is non-empty. It picks the MIME type from the **extension of
    `RelativeFilename || Filename`** (bmp, jpg/jpeg, png, tif, tga, webp), so the file name must carry a correct
    extension. It links the image to a Texture through the Texture's **first child connection** (Video→Texture OO).
  - **three.js crashes** (TypeError) if a `Texture` has no `FileName` [3js L443-488, L258-374].
  - **SDK [SDK-doc]:** *"only the binary FBX file format supports embedded media"*, and the export option
    `EXP_FBX_EMBEDDED` works only for binary output. On import, embedded media is **extracted to a `<name>.fbm\` folder**
    next to the file ("referencing_media" guide page). Maya, 3ds Max, Unity and Unreal all go through this path.
  - **Unity:** embedded textures are imported as sub-assets of the model. *"you can also extract them into your project
    using the Extract Textures button"*, which extracts *"all materials and textures that are embedded in your imported
    asset"* (https://docs.unity3d.com/Manual/FBXImporter-Materials.html).
  - **Blender:** the legacy importer reads `Content` and packs the images; there is also an "Image Search" option for
    external files. Partly UNVERIFIED (not read from code).
  - **Assimp:** exposes embedded images as `aiTexture` entries referenced as `*N`. **[V]**: Assimp *writes* Content when
    exporting a GLB with embedded images.
  - **Writer advice:** embed PNG or JPEG bytes in `Video.Content`. Give the Video and Texture identical
    `Filename`/`RelativeFilename` with a real image extension, e.g. `"textures/basecolor.png"`, so readers that only
    support external files still find a sensible path.

---------------------------------------------------------------------------------------------------------------------

## 5. Skinning

### 5.1 Records [V]
```
Deformer: L id, S "Name\x00\x01Deformer", S "Skin" { Version: I 101; Link_DeformAcuracy: D 50.0; [SkinningType: S "Linear"|"Rigid"|"DualQuaternion"|"Blend"] }
Deformer: L id, S "Name\x00\x01SubDeformer", S "Cluster" {
  Version: I 100
  UserData: S "", S ""
  Indexes: i[k]           control-point indices (omit both arrays for a bone with no influence; Mixamo does)
  Weights: d[k]           0..1 (SDK normalizes in eNormalize mode)
  Transform: d[16]        column-major 4x4
  TransformLink: d[16]
  [TransformAssociateModel: d[16]]   only for eAdditive link mode
  [Mode: S "Normalize"|"Additive"|"TotalOne"]  (absent = Normalize; the file spellings are UNVERIFIED. No SDK sample had Mode or TransformAssociateModel, and ufbx never reads Mode)
}
C "OO", skinId, geometryId        C "OO", clusterId, skinId        C "OO", boneModelId, clusterId
```
(The connection patterns were checked in two Mixamo files: Skin→Geometry, Cluster→Skin, LimbNode Model→Cluster, and Material→Model.)

Enums [SDK-doc]:
- `FbxCluster::ELinkMode`: eNormalize=0 (*"the sum of the weights assigned to a control point is normalized to 1.0"*),
  eAdditive=1 (*"the only mode where setting the associate model is relevant"*), eTotalOne=2 (*"not normalized and must
  equal 1.0"*). The docs add: *"All the links associated to an instance of class FbxGeometry must have the same link
  mode."*
- `FbxSkin::EType`: eRigid=0 (*"only one joint can influence each control point"*), eLinear=1, eDualQuaternion=2,
  eBlend=3. With eBlend, the per-vertex blend weight runs from 0 (linear) to 1 (dual quaternion).
- Link_DeformAcuracy is accuracy in percent (*"100% is full accuracy"*). The SDK writes 50.

### 5.2 What Transform and TransformLink mean (verified numerically) [V]
In Mixamo's SDK output the mesh node is at identity. Its BindPose entry is identity, and for `mixamorig:Hips`:
- `TransformLink` = rotation about X of 0.74° with translation (−7.7e-6, 104.27, 1.554). This equals the Hips' BindPose
  matrix, i.e. the **bone's global matrix at bind time**.
- `Transform` = rotation of −0.74° with translation (−4.45e-5, −104.286, −0.2073). This is **exactly TransformLink⁻¹**
  (the rotation is transposed and the translation is −Rᵀt, checked to 4 digits).

So the file stores `Transform = TransformLink⁻¹ · MeshGlobalAtBind`. That is the "geometry/mesh-node → bone" matrix, the
**inverse bind matrix**.

The `· MeshGlobalAtBind` factor was checked on two Blender-written skinned files whose mesh is **not** at identity
(Assimp test models `huesitos.fbx` and `animation_with_skeleton.fbx`). In each, `TransformLink⁻¹ · BindPose(mesh)` equals
`Transform` to within 1e-7 and 7e-7 respectively **[V]**.

The stored `Transform` is *not* what the SDK API's `GetTransformMatrix()` returns; the API converts when reading. At the
API level the SDK's SwitchBinding sample says: *"ThansformMatrix is the global transformation of the mesh when the binding
happens"* and *"TransformLinkMatrix is the global transformation of the joint(Link) when the binding happens"*
(https://help.autodesk.com/cloudhelp/2020/ENU/FBX-API-Reference/cpp_ref/_switch_binding_2main_8cxx-example.html).
The SDK's DrawScene sample deforms with `(MeshGlobalNow⁻¹·LinkGlobalNow)·(LinkBind⁻¹·MeshBind·Geometric)`. Readers must
use the file convention above.

**Further confirmation:**
- An agent ran the same check on an SDK-written file whose skinned mesh *is* transformed (ufbx test data
  `max_transformed_skin_7500`): `inv(TransformLink)·MeshBindPose` equals `Transform` to within 2.2e-8.
- ufbx reads `Transform` as `mesh_node_to_bone` and `TransformLink` as `bind_to_world`, and sets
  `geometry_to_bone = mesh_node_to_bone · geometry_to_node`, which its docs call the "inverse bind matrix"
  (`ufbx.c` ≈ L14027-14054, L23526-23531).
- ufbx fallbacks: an all-zero `Transform` becomes `inv(bind_to_world)·node_to_world`; an all-zero `TransformLink` is
  taken from the BindPose.

Assimp's exporter writes the same file convention **[V]**. For my joint J1 bound at y=+1, it wrote
`Transform = translate(0,−1,0)` (= glTF IBM) and `TransformLink = translate(0,+1,0)`. Its source (FBXExporter.cpp,
L1990-1993, L2205-2209) says:
- *"Transform and TransformLink elements, specifying the transform of the MESH in BONE space, and the transformation of
  the BONE in WORLD space, likely in the bindpose"*
- `bone_xform = mesh_xform * invOffsetMatrix; … AddChild("Transform", b->mOffsetMatrix); // TransformLink * Transform = mesh_xform`

**glTF ↔ FBX.** Choose `M` = the global matrix of the FBX mesh Model at bind time. glTF ignores a skinned mesh's node
transform, so any M works:
- **M = I**: put the skinned mesh at the root with no transform. This is the simplest choice.
- **M = the glTF mesh node's world matrix**: TransformLink then equals the joints' actual rest world matrices, *if* the
  glTF exporter built its IBMs with the mesh world.

For each joint j:
```
Transform_j     = IBM_j                      (for ANY choice of M: Transform maps vertex data space -> bone space, exactly like glTF's IBM)
TransformLink_j = M · IBM_j⁻¹                (joint world at bind; = IBM_j⁻¹ when M = I)
BindPose: mesh -> M, joint j -> TransformLink_j
Rendered (SDK/ufbx/three.js with BindPose): BoneWorldNow · Transform_j · v  ==  glTF JointWorldNow · IBM_j · v
FBX->glTF: IBM_j = Transform_j · G   (G = the mesh Model's Geometric* matrix; ufbx calls this geometry_to_bone)
```
How readers use these matrices:
- three.js **ignores `Transform`**. It sets each bone's bind `matrixWorld` from `TransformLink`, and takes the mesh bind
  matrix from the **BindPose entry for the mesh Model** [3js L811-850, L1461-1534]. Without a BindPose it binds against
  the mesh's matrix *before* node transforms are applied, effectively the identity.
- **Probe [V]** (`probe/skin_offset_{pose,nopose}.fbx`, `node/skin3.mjs`):
  - Setup: the skinned quad's Model is at x=+5, with `Transform = TL⁻¹·T(5,0,0)`. Bone1 sits at (0,1,0) and is
    animated 45° about Z.
  - **With** a BindPose holding the mesh matrix, three.js puts vertex (1,2,0) at (3.54, 5.95, 0). That is exactly a
    rotation of world point (6,2,0) about (0,1,0).
  - **Without** a BindPose, the same vertex lands at (0, 2.41, 0): the +5 mesh offset is lost.
  - **Conclusion: three.js needs a BindPose PoseNode for every skinned mesh whose global transform is not the identity.**
- Assimp's importer computes the bone offset as `TransformLink⁻¹ × (mesh node global)`, ignoring `Transform`. In the same
  probe Assimp gave Bone1 offset = T(5,−1,0), which equals the file's `Transform`, with or without the Pose **[V]**.
  Assimp still **requires both `Transform` and `TransformLink` to be present** (`FBXDeformer.cpp` ≈ L80-84), and it
  ignores Pose, Mode and TransformAssociateModel.
- OpenFBX exposes both raw matrices and reads only the first PoseNode of a Pose.

### 5.3 BindPose [V]
```
Pose: L id, S "Name\x00\x01Pose", S "BindPose" { Type: S "BindPose"; Version: I 100; NbPoseNodes: I n;
  PoseNode: { Node: L modelId; Matrix: d[16] (GLOBAL matrix, column-major) }  … }
```
Mixamo's BindPose lists the mesh(es) first (identity) and then every LimbNode. The bone matrices equal TransformLink.

The SDK docs (FbxPose class reference) say:
- *"In "Bind Pose" mode, the matrix is assumed to be defined in the global space"*
- The bind pose should cover *"the geometry being deformed, the links deforming the geometry, and recursively all the
  ancestors nodes of the link"*.
- A rest pose (`Type "RestPose"`) also exists, and its matrices may be local.
- **three.js:** optional; it uses only `PoseNode.Matrix` of the mesh Model, and only if `NbPoseNodes > 0`.
- **Assimp:** not needed (probe).
- **Assimp's exporter deliberately omits it.** Its comment (FBXExporter.cpp L2252-2259): *"This is a legacy system, which
  should be unnecessary. Somehow including it slows file loading by the official FBX SDK, and as it can reconstruct it
  from the deformers anyway, this is not currently included."* This is the Assimp developers' claim; it is not verified
  against the SDK.
- The same Assimp file also has disabled code commenting that *"the FBX SDK will complain if it's missing"*, so Assimp's
  own comments contradict each other.
- **ufbx** treats a BindPose as an optional fallback.
- **Blender, Maya, Unity:** no primary statement found on whether a missing BindPose is fatal: **UNVERIFIED**.

Recommendation: always write one BindPose that contains the mesh and **every** bone, plus any non-bone ancestors of bones,
as Maya does.

Influences: FBX has no limit on influences per vertex. three.js keeps the 4 largest and renormalizes [3js L1923-1975];
Unity defaults to 4. Normalize the weights before writing.

---------------------------------------------------------------------------------------------------------------------

## 6. Blend shapes / morph targets [V]
```
Deformer: L id, S "Name\x00\x01Deformer", S "BlendShape" { Version: I 100 }          C "OO", blendShapeId, geometryId
Deformer: L id, S "ChannelName\x00\x01SubDeformer", S "BlendShapeChannel" {
  Version: I 100
  DeformPercent: D 0.0          (0..100; current/static weight ×100)
  FullWeights: d[nShapes]       (100 for a single target; in-betweens: ascending % at which each shape is fully on)
}                                                                                     C "OO", channelId, blendShapeId
Geometry: L id, S "ShapeName\x00\x01Geometry", S "Shape" {
  Version: I 100
  Indexes: i[k]                 control-point indices that move
  Vertices: d[3k]               **deltas** relative to the base Vertices (not absolute)
  Normals: d[3k]                deltas of normals (SDK writes the array; zeros if unchanged)
}                                                                                     C "OO", shapeId, channelId
```
- **Deltas are confirmed** by the SDK 3ds Max file, whose `Vertices` hold (0, 3.2e-5, −400.0) offsets, and by three.js,
  which sets `morphTargetsRelative = true` and scatters the sparse deltas into a zeroed array [3js L2251-2320]. Normals in
  Shape records follow the shape's `Indexes` (per control point) for SDK output.
- **What the SDK writes** (Maya 7500 in-between sample, per the agent):
  - The Shape geometry has an empty `Properties70` with a NULL record, then `Version 100`, `Indexes`, `Vertices` and
    `Normals`.
  - The BlendShapeChannel has **both** `Properties70 { P "DeformPercent","Number","","A+",v }` **and** a child
    `DeformPercent: D v`, plus `FullWeights`, e.g. `50,100`, or `-49.9,-24.9,100` for negative in-betweens.
  - Shapes connect to the channel in `FullWeights` order.
- **Readers:**
  - **ufbx** requires `len(Indexes) == len(Vertices)/3` and `len(Normals) == len(Vertices)`. It uses the Properties70
    DeformPercent, falling back to the child (*"Blender saves blend shapes with DeformPercent as a field, not a
    property"*), and blends in-betweens linearly.
  - **Assimp** reads **only the child** `DeformPercent` and ignores FullWeights. It adds deltas and renormalizes normals.
  - **OpenFBX** reads the child and FullWeights.
  - **Writer:** write both forms of DeformPercent, FullWeights on the 0–100 scale, and a Normals array the same length as
    Vertices (zeros are fine).
- **Primary [SDK-doc]** (FbxShape, 2020 reference): *"Legacy style (default): … the control point position value and
  normal are saved as relative values to the base geometry values. Only normals for which the control point position has
  changed will be exported."* A "modern style" can write absolute values (`FbxShape::SetAbsoluteMode`), but relative is the
  default. Note that the SDK **API** exposes *absolute* shape control points and converts on save.
- **Channels and in-betweens [SDK-doc]** (FbxBlendShapeChannel):
  - `DeformPercent` defaults to 0.0, and Autodesk suggests limiting it to 0–100.
  - `AddTargetShape(shape, fullDeformPercent = 100)`. *"given a channel that has 3 target shapes, whose full deform
    percentage are 30, to 80 to 100 separately, then when the percent changes from 0 to 100, the base geometry will deform
    from the first target shape to the last one"*. The DrawScene sample interpolates linearly between adjacent shapes.
  - FbxBlendShape: *"The blend effect of each blend shape channel is additive."*
- **Animation:** an `AnimationCurveNode` named **"DeformPercent"**
  (`S "DeformPercent\x00\x01AnimCurveNode"`, `Properties70 { P "d|DeformPercent","Number","","A",v }`):
  - `C "OP", curveNodeId, channelId, "DeformPercent"`
  - `C "OP", curveId, curveNodeId, "d|DeformPercent"`
  - `C "OO", curveNodeId, layerId`

  Values are 0–100; three.js divides by 100 [3js L2945-2958].
- **three.js requirements:**
  - Each channel must have `FullWeights`, or the loader crashes reading `.a` [3js L866-868].
  - The Shape must be connected `OO` to the channel. Only the first Shape is used, so in-betweens are ignored.
  - Morph normals are not loaded.
  - For the animation track, the channel's first parent must be the BlendShape, the BlendShape's first parent the
    Geometry, and the Geometry's first parent the Model [3js L2718-2747].
  - The static `DeformPercent` is read but not applied as the initial influence.
- **glTF→FBX:** for each target i, write a channel with `DeformPercent = 100·mesh.weights[i]`, in both the Properties70
  and the child, `FullWeights = [100]` and one Shape with only non-zero deltas. Name the channels after
  `mesh.extras.targetNames`, as Assimp does **[V]**. glTF morph targets on several primitives of one mesh become one
  Shape per FBX geometry. Their indices refer to the control points of the merged FBX mesh.

---------------------------------------------------------------------------------------------------------------------

## 7. Animation [V]
```
AnimationStack: L id, S "Name\x00\x01AnimStack", S "" { Properties70: { P "LocalStart","KTime","Time","",t0; P "LocalStop",…,t1;
                                                                       P "ReferenceStart",…; P "ReferenceStop",… } }        (+NULL)
AnimationLayer: L id, S "Name\x00\x01AnimLayer", S ""            (no children, but MUST end with a NULL record, §1.2)
AnimationCurveNode: L id, S "T\x00\x01AnimCurveNode", S "" { Properties70: { P "d|X","Number","","A",x; P "d|Y",…; P "d|Z",… } }
AnimationCurve: L id, S "\x00\x01AnimCurve", S "" {
  Default: D v                  KeyVer: I 4009 (SDK; Blender writes 4008)
  KeyTime: l[n]                 int64 ticks, 46,186,158,000 per second
  KeyValueFloat: f[n]           float32 values (degrees for rotation)
  KeyAttrFlags: i[m]            per attribute run
  KeyAttrDataFloat: f[4m]       per attribute run
  KeyAttrRefCount: i[m]         run lengths; sum == n
}
C "OO", layerId, stackId; C "OO", curveNodeId, layerId; C "OP", curveNodeId, modelId, "Lcl Translation"|"Lcl Rotation"|"Lcl Scaling";
C "OP", curveId, curveNodeId, "d|X"|"d|Y"|"d|Z"
```
- **Ticks.** The SDK Mixamo file keys at 30 fps are spaced exactly 1,539,538,600 = 46,186,158,000/30 apart, and three.js
  divides by 46186158000 **[V]**. `GlobalSettings.TimeMode` was 6 at 30 fps (Mixamo) and 11 at 24 fps (Blender,
  `CustomFrameRate 24`).
- **Time base change in SDK 2019.5 [SDK-doc]** (G19 `what_new/fbx_sdk_2019.html`, `fbxtimecode.h`, `fbxio.h`):
  - Quote: *"Constant FBXSDK_TC_MILLISECOND has been changed from 46186158 to 141120 to accurately support frame rates
    like ~23.976, ~29.97, 72 and 96 fps … By default, we keep using the old value (FBXSDK_TC_LEGACY_DEFINITION) and you
    'opt in' for the new one."*
  - Constants: `FBXSDK_TC_LEGACY_DEFINITION 127`, `FBXSDK_TC_STANDARD_DEFINITION 0`. `FBX_FILE_VERSION_7600` exists for
    files *"written using the new FBXSDK_TC_MILLISECOND but without the flag to identify this"*.
  - Where the flag sits: `FBXHeaderExtension { OtherFlags { TCDefinition: I 127 } }` in the 7700 Mixamo file **[V]**. The
    127 value matches the legacy constant, and the key spacing there is legacy.

  **What ufbx does** (`ufbxi_read_header_extension`, ufbx.c ≈ L12017-12026, master 5955c5c). Its comment reads: *"FBX 8000
  will change the KTime units and the new units are opt-in currently via `TCDefinition` … as long as
  `FBXHeaderVersion >= 1004`. The old KTime units are specified as the value `127` and all other values seem to use the
  new definition."* The code is
  `use_v7 = version < 8000; if (FBXHeaderVersion >= 1004 && has TCDefinition) use_v7 = (TCDefinition == 127); ktime_sec = use_v7 ? 46186158000 : 141120000`.
  OpenFBX (`ofbx.cpp` ≈ L511-520) and Assimp (`FBXCommon.h` L61, `SECOND = 46186158000`) ignore the flag and hard-code
  the legacy rate.

  **Reader rule (follow ufbx):**

  | file | ticks per second |
  |---|---|
  | version < 8000, no `OtherFlags.TCDefinition` (or FBXHeaderVersion < 1004) | 46,186,158,000 |
  | `FBXHeaderVersion ≥ 1004` with `TCDefinition = 127` | 46,186,158,000 |
  | `FBXHeaderVersion ≥ 1004` with any other `TCDefinition` | 141,120,000 |

  The SDK's `fbxio.h` comment says version-7600 files use the new rate *without* the flag, but ufbx does not special-case
  7600. Such files are described as "the few existing files"; treat them as rare.

  **Writer:** emit `FBXHeaderVersion 1003`, 7400/7500, 46,186,158,000 ticks/s and no `OtherFlags`. If you ever write
  `FBXHeaderVersion 1004`, also write `OtherFlags { TCDefinition: I 127 }`. Never write any other value, because ufbx
  would switch rate while Assimp and OpenFBX would not.
- **TimeMode enum [SDK-doc]** (`FbxTime::EMode`, SDK 2020):

  | value | mode | value | mode |
  |---|---|---|---|
  | 0 | eDefaultMode | 10 | ePAL (25) |
  | 1 | 120 fps | 11 | 24 fps |
  | 2 | 100 fps | 12 | 1000 |
  | 3 | 60 fps | 13 | eFilmFullFrame (23.976) |
  | 4 | 50 fps | 14 | **eCustom** (use `CustomFrameRate`) |
  | 5 | 48 fps | 15 | 96 fps |
  | 6 | 30 fps | 16 | 72 fps |
  | 7 | 30 drop | 17 | 59.94 fps |
  | 8 | NTSC drop 29.97 | 18 | 119.88 fps (2019.5+) |
  | 9 | NTSC full 29.97 | | |

  `CustomFrameRate` is *"meaningless if the time mode is not FbxTime::eCustom"* (write -1). `TimeProtocol`:
  0 SMPTE, 1 frame count (default), 2 default.
- **Curve node names:** "T", "R" and "S". three.js keys on these *names*, not on the OP property [3js L2548-2576, L2828-2847].
  The OP property for scale is **"Lcl Scaling"**. Assimp 6.0 writes "Lcl Scale", which is an exporter bug **[V]**.
- **KeyAttrFlags bits** [SDK-doc, `FbxAnimCurveDef`, identical in 2019 and 2020; ufbx_write uses the same values]:

  | bit | name | SDK doc text |
  |---|---|---|
  | 0x2 | eInterpolationConstant | *"Constant value until next key"* |
  | 0x4 | eInterpolationLinear | |
  | 0x8 | eInterpolationCubic | |
  | 0x100 | eTangentAuto | *"Auto key (spline cardinal)"*. **Same bit as eConstantNext** (constant interpolation: hold the next key's value) |
  | 0x200 | eTangentTCB | |
  | 0x400 | eTangentUser | *"Next slope at the left equal to slope at the right"* |
  | 0x800 | eTangentGenericBreak | *"Independent left and right slopes"* |
  | 0xC00 | eTangentBreak | |
  | 0x900 | eTangentAutoBreak | |
  | 0x1000 | eTangentGenericClamp | |
  | 0x2000 | eTangentGenericTimeIndependent | |
  | 0x6000 | eTangentGenericClampProgressive | |
  | 0x1000000 / 0x2000000 | eWeightedRight / eWeightedNextLeft | |
  | 0x10000000 / 0x20000000 | eVelocityRight / eVelocityNextLeft | |
  | 0x100000 / 0x200000 | tangent show left / right | UI only |

  **One attribute describes the segment from key i to key i+1.** "Right" is key i's outgoing tangent and "NextLeft" is key
  i+1's incoming tangent. ufbx_write builds each attribute this way (`ufbxwi_get_key_flags(prev, next)`).
  Curves may also carry `Pre-Extrapolation` and `Post-Extrapolation` blocks:
  `{ Type: C 'C'|'R'|'M'|'K'|'A'; Repetition: I n }` = constant, repeat, mirror, keep-slope, relative-repeat. They are
  omitted when constant (ufbx_write).

  The SDK's own ASCII writer annotates `8456 = 0x2108` as `Cubic|TangeantAuto|GenericTimeIndependent`
  (`ball_anims_asc_2018.fbx`) **[V]**. Mixamo writes the same value for all keys, with one run. Blender writes
  `24836 = 0x6104` (linear|auto|time-independent|clamp-progressive) **[V]**.
- **KeyAttrDataFloat:** 4 float32 per run, `[RightSlope, NextLeftSlope, packedWeights, packedVelocity]`.
  `packedWeights` is a float32 whose **bit pattern** is two u16 fixed-point weights: right weight in the low 16 bits and
  next-left weight in the high 16 bits. This matches ufbx_write's `pack(right) | pack(next_left) << 16`.
  - The SDK writes `0x0D050D05` (3333, 3333), the default 1/3 weights. Its doc says *"Tangent has default weights of
    0.333"*, and `EDataIndex eWeights` notes *"data are FbxInt16 tokens from weight and converted to float"*.
  - Divider: the legacy SDK header says `KFCURVE_WEIGHT_DIVIDER 9999 // precise enough and can be divided by 3 without
    error`; ufbx_write uses 10000. Both give 3333 for 1/3.
  - That float is 4.0999461e-31 in binary files. The SDK's ASCII writer prints the raw bits as the integer `218434821`
    **[V]**.
  - Write `[0, 0, f32::from_bits(0x0D050D05), 0]`.
- **KeyAttrRefCount:** `[n]` with one shared attribute run is what Mixamo and Assimp write; per-key `[1,1,…]` is what
  Maya ASCII wrote. Both are valid.
- **Readers:**
  - **three.js** ignores KeyAttrFlags, so everything plays as linear (quaternion slerp for rotations). It also ignores
    `Default`.
  - **Assimp** ignores the flags and lerps (`FBXConverter.cpp` ≈ L3559-3600). It **requires strictly increasing
    `KeyTime`** ("the keyframes are not in ascending order"), so never write duplicate times.
  - **OpenFBX** is linear only.
  - **ufbx** evaluates the flags fully: constant, linear, cubic auto/user/TCB, and weighted Bezier with the tangent
    construction at `ufbx.c` ≈ L14498-14516. It **requires all five arrays** (`KeyTime`, `KeyValueFloat`, `KeyAttrFlags`,
    `KeyAttrDataFloat`, `KeyAttrRefCount`); a missing one fails the load. It also checks
    `len(KeyTime) == len(KeyValueFloat)`, `len(KeyAttrFlags) == len(KeyAttrRefCount)`,
    `len(KeyAttrDataFloat) == 4·len(KeyAttrRefCount)`, and that `sum(KeyAttrRefCount)` covers all keys.
  - **Flag values seen in SDK files:** Maya writes `1028` (linear|user), `1032` (cubic|user), `50332680`
    (cubic|user|weighted right and next-left), `2` (constant) and `258` (constant|next). Assimp 6.0 wrote `0` **[V]**;
    Assimp master writes `8452` (0x2104, linear|auto|time-independent).
  - **Weight decoding:** ufbx reads the packed weights ×0.0001 (*"two 0.4 _decimal_ fixed point values that are packed
    into 32 bits and interpreted as a 32-bit float"*).
- **three.js hard requirements:**
  - An `R` curve node must have **all three** of d|X, d|Y and d|Z; otherwise the track is a dummy.
  - The three rotation curves must have **identical key counts and times**, because they are walked by the X curve's
    index [3js L2869-2943, L3067-3171].
  - A curve's first connection must be to its curve node, and a stack's first child must be its layer. Only one layer per
    stack is used.
- **Rotation order:** keyed values are `Lcl Rotation` Euler degrees, interpreted with the node's `RotationOrder`. Pre/post
  rotation is applied as in §3.2.
- **Quaternion sources, glTF→FBX:**
  1. Convert each key to XYZ Euler.
  2. **Unwrap** each channel by adding ±360 to minimize jumps from the previous key.
  3. Because linear Euler ≠ slerp, **resample** at the source frame rate, or adaptively wherever the angular error exceeds
     a tolerance.

  three.js inserts slerp sub-steps whenever a channel jumps ≥180° between keys, but other importers do not. Near the
  ±90° pitch singularity, Euler decomposition flips. Unwrapping fixes ±360° discontinuities but not the
  gimbal-induced 180° flips of the other two axes, so resample densely there.

  Blender's exporter always **bakes** (`bake_anim=True`, `bake_anim_step=1.0` frame, `bake_anim_simplify_factor=1.0`).
  Its option `bake_anim_use_all_bones` is described as *"Force exporting at least one key of animation for all bones
  (needed with some target applications, like UE4)"*. Its importer applies `use_prepost_rot=True` by default.
  - FBX→glTF: Blender's exporter `add_leaf_bones=True` appends extra `_end` leaf bones, which a converter may want to
    drop.
  - glTF→FBX: write at least one key for every animated joint's T, R and S in every stack, as Blender does for UE4.
- **STEP to FBX:** constant interpolation (`0x2`). **CUBICSPLINE to FBX:** either bake to linear keys (recommended) or use
  user tangents (`0x8|0x400`, plus `0x800` "break" when the in and out slopes differ) with slopes in KeyAttrDataFloat.
  ufbx treats slopes as **value units per second** and builds tangent handles as `dx = weight·Δt`, `dy = dx·slope`.
  glTF CUBICSPLINE tangents are also per second, so with the default weights 1/3 they map directly to FBX user slopes
  (this mapping is an inference). Baking remains the robust option, because three.js, Assimp and OpenFBX are linear-only.
- **Takes** should mirror each stack: `Take: S name { FileName: S "name.tak"; LocalTime: L, L; ReferenceTime: L, L }`.
  Also set `Documents…ActiveAnimStackName` and `GlobalSettings.TimeSpanStart/Stop`.

---------------------------------------------------------------------------------------------------------------------

## 8. Axis and unit conventions

**GlobalSettings semantics.**
- **Axes.** Each of `UpAxis`, `FrontAxis` and `CoordAxis` is an **axis index 0 = X, 1 = Y, 2 = Z** plus a `…Sign` of ±1.
  The FbxGlobalSettings doc for `GetOriginalUpAxis` says *"0 is X, 1 is Y, 2 is Z axis"*. ufbx and Assimp decode them the
  same way.
- The SDK's in-memory `FbxAxisSystem` enums differ: `EUpVector` eXAxis=1 / eYAxis=2 / eZAxis=3 (signed);
  `EFrontVector` eParityEven=1 / eParityOdd=2 (*"the first one and the second one of the remain two axes"*);
  `ECoordSystem` eRightHanded=0 / eLeftHanded=1. Do not confuse these with the file integers.
- **Presets** [SDK-doc]:
  - MayaYUp and OpenGL: up +Y, front +Z, coord +X (right-handed).
  - MayaZUp and Max: up +Z, front −Y, right-handed.
  - DirectX and Lightwave: up +Y, front +Z, coord −X (left-handed).

  Sample files: Maya writes Up 1/+1, Front 2/+1, Coord 0/+1. 3ds Max writes Up 2/+1, Front 1/−1, Coord 0/+1.
  ufbx's comment: *"`front` is the _opposite_ from forward!"*
- **Units.** `UnitScaleFactor` is **centimetres per file unit**. `FbxSystemUnit`'s scale factor is *"The equivalent number
  of centimeters in the new system unit. For example, an inch unit uses a scale factor of 2.54"*. The values are
  cm = 1, m = 100, mm = 0.1, in = 2.54. ufbx computes `unit_meters = UnitScaleFactor × 0.01`.
  `OriginalUnitScaleFactor`, `OriginalUpAxis` and `OriginalUpAxisSign` record the authoring application's values; write
  them equal to the current ones.
- **SDK behaviour** [SDK-doc, "Scene axis and unit conversion"]:
  - *"Objects in the FBX SDK are always created in the right handed, Y-Up axis system."*
  - *"The implicit coordinate system is in centimeters."*
  - *"calls to ConvertScene() do not change the vertex values of meshes, and only affect node transforms and
    animations"*. Converters therefore act at the root nodes.

**A Y-up, right-handed, metre-scale glTF scene.**
- glTF is *"right-handed… +Y as up, +Z as forward, and -X as right"*, with the asset's front facing +Z.
- That is the same frame as FBX MayaYUp. FBX's CoordAxis +X is the viewer's right; glTF's −X right is the asset's right.
  So no vertex or rotation change is needed. Write:
  `UpAxis 1, UpAxisSign 1, FrontAxis 2, FrontAxisSign 1, CoordAxis 0, CoordAxisSign 1, OriginalUpAxis 1, OriginalUpAxisSign 1`.
  These are exactly the values Mixamo and Assimp write **[V]**.
- Unit options:
  - **(a)** Keep metre data and write `UnitScaleFactor = OriginalUnitScaleFactor = 100`. This is raw values plus a
    declared unit, as Autodesk plug-ins and Blender's "FBX Units Scale" do.
  - **(b)** Bake ×100 into all positions, translations, bind and pose matrices, animation translation keys and shape
    deltas, and write `UnitScaleFactor = 1` (cm). This is the Maya-default and Mixamo convention.
  - **Avoid** Blender's default "All Local" style: metre data with ×100 scale on root nodes and USF 1, which is the source
    of most "scale 100" complaints (Blender issue #70161).

**What importers do.**

| importer | axes | UnitScaleFactor | (a) m + USF 100 | (b) cm + USF 1 |
|---|---|---|---|---|
| **three.js r183** (local, **[V]** probe) | ignored | stored in `userData.unitScaleFactor`, **never applied** | correct size | 100× too big |
| **three.js dev 2026** (**[V]** source) | if `UpAxis==2`, rotates the root −90° about X; vertices not converted | same as r183 | correct | 100× too big |
| **Assimp 6.0 CLI** (**[V]** probe) | **axis correction applied at the root**: a Z-up file got a −90° X root matrix | **not applied**; root identity for USF 1 and 100 (metadata only) | correct (raw m) | 100× (raw cm) |
| **Unity** | converts to Y-up left-handed by negating X. `Bake Axis Conversion` bakes this into the data; otherwise it is on the root object | `fileScale` = USF/100 with Convert Units / Use File Scale | correct | correct |
| **Maya / 3ds Max plug-ins** | automatic conversion at root nodes; root animation is resampled | "Automatic" unit conversion to the host unit | correct | correct |
| **Blender (legacy importer)** | uses file axes unless "Manual Orientation" | uses the file scale ("Apply Unit") | correct | right size, but roots get 0.01 scale |
| **Unreal** (left-handed, Z-up, cm) | "Convert Scene" | "Convert Scene Unit" | **risk: 100× too small unless Convert Scene Unit is on** (2017 report; current defaults UNVERIFIED) | correct |

Sources:
- Unity: https://docs.unity3d.com/ScriptReference/ModelImporter-fileScale.html and
  https://docs.unity3d.com/ScriptReference/ModelImporter-bakeAxisConversion.html
- Unreal: https://dev.epicgames.com/documentation/en-us/unreal-engine/fbx-import-options-reference-in-unreal-engine
- Blender: https://docs.blender.org/api/current/bpy.ops.import_scene.html and https://projects.blender.org/blender/blender-addons/issues/51704
- Maya: https://help.autodesk.com/cloudhelp/2016/ENU/Maya/files/GUID-3EDA2660-DB47-4DEE-A726-12EF26B11EB7.htm

**Assimp nuance** (code: master `FBXConverter.cpp` L79-123 and L3729, read here; BSD).
- `correctRootTransform` (Assimp ≥ 5.4, PR #5494; opt-out `AI_CONFIG_IMPORT_FBX_IGNORE_UP_DIRECTION`) multiplies the root
  by rows {right, up, forward} built from the axis metadata. Each row is *intended* to be scaled by `UnitScaleFactor`, read
  via `Get<double>`.
- But the metadata is stored from `GlobalSettings().UnitScaleFactor()`, which is a float. The double lookup therefore
  fails and the scale stays 1.0. This explains my measurement: axis correction happens, unit scaling does not. The
  explanation is an inference from code plus that test.
- `AI_CONFIG_FBX_CONVERT_TO_M` is read but unused in the FBX code. The file scale reaches only `aiProcess_GlobalScale`
  (via `AI_CONFIG_APP_SCALE_KEY`), if the application enables it.

**ufbx** performs **no conversion by default** (*"By default units are not scaled"*). Conversion is opt-in through
`target_axes`, `target_unit_meters` and `space_conversion` (`TRANSFORM_ROOT`, `ADJUST_TRANSFORMS` or `MODIFY_GEOMETRY`).
It also notes that FBX cameras look along +X and lights along −Y.

**Recommendation.** For a glTF hub, default to **(a) metres + `UnitScaleFactor 100`**. three.js and Assimp ignore USF, so
raw metres keep them identical to the glTF, and Unity, Blender and the Autodesk DCCs convert correctly. Offer **(b)** as an
"Unreal/Maya cm" profile. Keep unit scale in the vertex data, never as a node or bone scale; scaled root bones break
Unreal sockets and retargeting.

**Z-up sources (FBX→glTF).** Apply one root rotation of −90° about X, or the general basis change built from Up/Front/Coord,
plus the scale `UnitScaleFactor × 0.01` to get metres. three.js dev and Assimp apply only the rotation, not the unit
scale. Never bake it into bones unless you also convert the bind matrices.

---------------------------------------------------------------------------------------------------------------------

## 9. Licensing (not legal advice)
**This is research for counsel, not legal advice.**

**Autodesk FBX SDK licence.** The text is "Autodesk LICENSE AND SERVICES AGREEMENT – Autodesk® FBX® SDK 2020", with
identical public copies at https://unity.com/legal/autodesk-fbx and https://www.sidefx.com/docs/houdini/licenses/fbx.html.
- **Who is bound.** It binds only whoever accepts it, and *"by installing, downloading, accessing, or otherwise copying
  or using"* the materials, someone accepts *"on behalf of the entity for which you are authorized to act (e.g., an
  employer)"*. One engineer downloading the SDK can therefore bind the company.
- **Shipping it in your product is allowed.** It may be linked into "Licensee Products" and redistributed inside them
  "as is" (§1.1.1–1.1.3), with a mandatory credit notice (§1.1.5). Autodesk's docs say the SDK *"cannot be redistributed
  or repackaged without written permission"* on its own.
- **Restrictions in §2.1.1:**
  - (i) no *"decompile, disassemble or otherwise reverse engineer the Autodesk Materials"*;
  - (j) no derivative works;
  - **(f)/(g)** no making the SDK's *"features or functionality … available … whether or not over a network … on a hosted
    basis"* and no use for *"software as a service, cloud or other service"*.

  Clauses (f)/(g) make running the SDK **server-side in a SaaS** questionable without a separate Autodesk deal. That is a
  point in favour of a from-scratch implementation.
- **No clause restricts the FBX file format itself.** There is no "use FBX only via the SDK" term. Autodesk's docs merely
  advise: *"The FBX file format is not documented. Applications should use the FBX SDK…"*
- The reverse-engineering ban covers the SDK binaries, not FBX files. Studying files produced by tools is a different
  activity; that is the agent's reading and is UNVERIFIED legally. In the EU, Software Directive 2009/24 art. 5(3) and
  art. 8 void contractual bans on observing or studying a program.

**Writing FBX without the SDK.**
- **Patents:** none known. The US Library of Congress format record (https://www.loc.gov/preservation/digital/formats/fdd/fdd000558.shtml)
  says *"None known for file format"* and calls FBX *"a closed format"*. The one Autodesk patent it cites, US 7,254,235,
  shows as expired in 2023. No freedom-to-operate search was done.
- **Copyright:** data formats and functionality are not protected expression. EU: CJEU C-406/10 *SAS v WPL* ¶39. US:
  17 U.S.C. §102(b). Infringement risk comes only from copying code or text: SDK headers/samples, Autodesk doc text
  (CC BY-NC-SA) or **GPL** code such as Blender's exporter.
- **Trademark:** "FBX" is a live Autodesk US registration (Reg. 2,676,937); "KAYDARA" is cancelled. Referential use such as
  "export to FBX" or "FBX-compatible" is fine under Autodesk's own trademark guidelines (*"Use a referential phrase like
  'for,' 'for use with,' 'compatible with'"*). Don't use FBX in a product name, and write your own `Creator` and
  `ApplicationVendor` strings, never Autodesk ones.
- **Known legal action:** none found. Searches covered lawsuits, cease-and-desist letters and DMCA notices against
  Blender, Assimp, Godot, OpenFBX and ufbx, plus all 14 Autodesk notices in github/dmca. Blender's FBX IO has been
  reverse-engineered and public since 2007.

**Permissively licensed implementations that do not use the SDK:**

| project | licence | read/write | notes |
|---|---|---|---|
| **Assimp** (C++) https://github.com/assimp/assimp | BSD-3 | R+W | writes binary 7500 and ASCII (`fbx`/`fbxa`) |
| **ufbx_write** (C) https://github.com/ufbx/ufbx-write | MIT or Unlicense | W | binary and ASCII, default 7500. By the ufbx author; v0.2.0, "work-in-progress". Implements the FileId/footer hashes (§1.4). **Best reference writer.** |
| **ufbx** (C) https://github.com/ufbx/ufbx | MIT or Unlicense | R | reads ASCII and binary ≥ 3000; Rust crate `ufbx` (reader only); used by Godot 4.3+ and Blender 4.5+'s new importer |
| **OpenFBX** (C++) https://github.com/nem0/OpenFBX | MIT | R | |
| **fbxcel** (Rust) https://github.com/lo48576/fbxcel | MIT/Apache-2.0 | R + low-level W | binary 7.4/7.5 only, dormant since 2022 |
| draco-io (Rust) | Apache-2.0 | R+W | new and immature |
| SmallFBX (C++) | MIT | R+W | stale |
| three-js-fbx-exporter (TS) | MIT | W | new |

**Copyleft:** Blender's `io_scene_fbx` is GPL-2.0-or-later (writes binary 7400). Do not copy or port it into proprietary
code.

**Uses the SDK, so not a clean option:** FBX2glTF, fbx-conv, Unity FBX Exporter, fbxsdk-sys.

**Risk summary (agent's assessment, not legal advice): LOW** for a SaaS that emits FBX files from its own code, provided
the team:
- keeps the SDK and Blender's GPL code away from implementers;
- copies no Autodesk doc text;
- uses referential trademark wording;
- does not implement decryption of password-protected FBX (`EncryptionType`), which carries a possible DMCA §1201
  anti-circumvention issue.

---------------------------------------------------------------------------------------------------------------------

## 10. AI 3D vendors returning rigged/animated models
Tags: [DOC] = vendor docs. [OBS] = the research agent parsed real public output files; this is not vendor-documented,
so it is UNVERIFIED for generality.

| vendor | rig/animate API | outputs | what the files contain |
|---|---|---|---|
| **Meshy** [DOC] https://docs.meshy.ai/api/rigging.md, /api/animation.md | `POST /openapi/v1/rigging` (textured humanoid biped; `height_meters` default 1.7; GLB input faces +Z). `POST /openapi/v1/animations` (`action_id`, up to 10 `action_ids` merged into one file, or `motion_task_id`; library IDs 0–696). Post-processing `change_fps` 24/25/30/60 (default 30), `fbx2usdz`, `extract_armature` | `rigged_character_fbx_url` / `_glb_url`, `basic_animations` (walk/run), `animation_fbx_url` / `_glb_url` | **[OBS] FBX:** 7400 written by *Blender FBX IO 4.0*, Y-up, USF 1 (cm), root null "Armature" with **Lcl Rotation (−90,0,0)**. **24 bones**, Mixamo-*like* names **without** the `mixamorig:` prefix and no fingers. Base-colour PNG **embedded** (Video Content). **Up to 8–10 influences/vertex** in FBX vs 4 in GLB. 30 fps. Marketing says "Bone names follow Mixamo's conventions". |
| **Tripo** [DOC] https://developers.tripo3d.com/en/docs/animations-rig.md | v3: `POST /v3/animations/rig-check`, `/v3/animations/rig`, `/v3/animations/retarget` (v2 `animate_rig` / `animate_retarget` being shut down). `rig_type` biped/quadruped/hexapod/octopod/avian/serpentine/aquatic; `spec` `tripo`\|`mixamo`; `out_format` glb\|fbx; presets (`preset:walk` …). Convert has `fbx_preset` blender/3dsmax/mixamo and `export_orientation` (default +X) | GLB (default) or FBX | **[OBS]** spec=tripo has 41 joints (Root, Hip, Pelvis, … twist bones, no fingers); spec=mixamo has 23 joints ("Root" + 22 `mixamorig:` names). Max 4 influences. Embedded JPEG. **Model faces +X and is normalised to ~1 unit**, not real height. FBX fps, units and embedding are undocumented. |
| **Tencent Hunyuan3D** [DOC] https://cloud.tencent.com/document/api/1804/131618 | `SubmitAutoRiggingJob` / `DescribeAutoRiggingJob`. Input FBX/GLB ≤ 60 MB, A/T-pose humanoid (non-humanoid allowed but without motion templates); `MotionType` presets 1–48 | `ResultFile3Ds [{Type:"FBX", Url}]` (1-day URL) | skeleton, fps and units undocumented |
| **Anything World** [DOC] https://anything-world.gitbook.io/anything-world/api/rest-api-references | `POST /animate` (rig + animate), `/rig`, `/animate-processed` ("experimental") | FBX + GLB (also DAE/GLTF) | per-category clip sets; skeleton undocumented |
| **fal.ai** [DOC] https://fal.ai/models/fal-ai/meshy/rigging/llms.txt | `fal-ai/meshy/rigging` (+ `/multi-animation`): Meshy-backed, same outputs. `fal-ai/hunyuan-motion` gives text→FBX motion | GLB/FBX | as Meshy |
| **Replicate** | community UniRig (`aaronjmars/unirig-ai`): skeleton + skinning for any mesh; `uthana/create-character-v1` (biped) | GLB/FBX | **[OBS]** UniRig: generic `Bone_000…` names, up to 10 influences (JOINTS_0..2) |
| **No rigging API** | Hyper3D Rodin (only a `TAPose` flag), Stability SF3D/SPAR3D (static GLB), Luma, Kaedim, Sloyd (rigging "coming soon"), Masterpiece X, CSM | | |
| **Mixamo** (Adobe, **no public API**) | web auto-rigger (bipedal humanoids) + mocap library | Download as FBX Binary (`fbx7_2019`, default), FBX ASCII, FBX for Unity, FBX 7.4, FBX 6.1 or DAE; fps 24/30/60 (default **30**); With/Without Skin; keyframe reduction | **[V]+[OBS]:** current downloads are **FBX 7700 written by SDK 2020.2** (older ones 7400 / SDK 2014). Y-up, USF 1 (**cm**), TimeMode 6 (30 fps), `mixamorig:` prefix. **65 bones** with fingers (25 body + 40 finger; 67 with eyes; LODs 49/41/25). PreRotation on joints, InheritType 2. Textures **embedded** as PNG Content (diffuse/specular/gloss/normal, Phong). Up to 7–8 influences. Takes "mixamo.com" + "Take 001". |

**Converter hazards these vendors create:**
- More than 4 influences per vertex. glTF allows JOINTS_1/WEIGHTS_1, but many runtimes keep only 4, so prune and
  renormalise.
- cm units with USF 1 (Mixamo, Meshy).
- A −90° X root rotation (Meshy's Blender-made FBX).
- +X-facing models normalised to about 1 unit (Tripo).
- `PreRotation` and `InheritType 2` on joints (Mixamo), which must be baked when going FBX→glTF.
- FBX 7700 input (Mixamo) needs the 64-bit record layout and the TCDefinition handling of §7.
- Embedded textures appear in almost every vendor FBX.

---------------------------------------------------------------------------------------------------------------------

## Consolidated UNVERIFIED items (test these before relying on them)
1. Whether the Autodesk SDK **checks the FileId / CreationTime / footer-id hashes**. ufbx issue #229 points to yes. Use
   the constant triple, or compute the hashes (§1.4).
2. Whether the SDK accepts a file with **no footer at all**.
3. Which top-level records, and whether `Edges` or `BindPose`, are **required by SDK-based importers** (Maya, Max, Unity,
   Unreal). Write all of them.
4. Unreal's default for "Convert Scene Unit": does metres + `UnitScaleFactor 100` import at the right size by default?
5. The **colour space of material colours**: three.js treats them as sRGB, Unity URP as linear. Maya's behaviour is unknown.
   Vertex colours are sRGB per Blender and three.js.
6. How Maya, Unity and ufbx react to Stingray `Maya|…` properties on a `phong` material **without** the
   `Implementation`/`BindingTable` objects, and with them but without a ShaderGraph blob.
7. Cluster `Mode` string spellings ("Normalize"/"Additive"/"TotalOne"): never seen in SDK output, and ufbx ignores them.
8. The meaning of `'T'` vs `'Y'` in `Model.Shading` (`C` type), and fbxcel's claim that 0x00/0x01 bools are ignored by
   the SDK.
9. The SDK's exact array-compression threshold: it lies between 939 and 1072 bytes in my samples.
10. The `Texture_Alpha_Source` string for eRGBIntensity, and the `FbxFileTexture` file-level property names
    ("Texture alpha", "TextureRotationPivot"). These were seen in Assimp's template, not in SDK docs.
11. Vendor file details marked [OBS] in §10: Meshy, Tripo and UniRig skeleton, influence and scale specifics come from a
    handful of public sample files.
