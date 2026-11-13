#!/usr/bin/env python3
"""The reference corpus's glTF reading, manifest and cross-check.

mesh-gaps item 2, Task 4b; README.md beside this file says how
make_reference.sh runs each step. litegen's code, Python 3 standard library
only; run it as `python3 -I`.

  reference.py geometry <out_dir> <name>=<file.gltf|file.glb> ...
      Every triangle primitive of each file as <out_dir>/<input>.geom, the
      geometry file encode_reference reads, and <out_dir>/inputs.tsv, a row
      an input. A primitive whose points, faces and values equal one already
      taken is left out, and inputs.tsv names the one it repeats.

  reference.py store <all_dir> <streams_dir> <pairs.tsv>
      Every byte-distinct stream of <all_dir> (every input at every setting)
      once, into <streams_dir>, under the first (input, setting) pair that
      wrote it; and <pairs.tsv>, a row a pair naming its stored stream
      (Ruling B42: -qp is a no-op on an input whose positions are integers,
      so its streams repeat across the position bits).

  reference.py manifest <streams_dir> <dumps_dir> <geometry_dir> <pairs.tsv> <manifest> <streams.inc>
      The manifest draco_core_matches_googles_decoder reads: for each stored
      stream <streams_dir>/<input>.<setting>.drc, Google's decoding as
      dump_reference left it in <dumps_dir>/<stream>/, hashed, and a `same`
      row for every other pair whose stream is that file, byte for byte. And
      <streams.inc>, the Rust listing (name, include_bytes!) of every stored
      stream, which draco.rs include!s.

  reference.py equivalence <dumps_dir> <pairs.tsv>
      For every input, Google's decoding of its seqc-qp11 stream against its
      decoding of its cl0-qp11 stream, byte for byte (every attribute dump
      and the connectivity), where it decodes both: the evidence behind the
      manifest's `google-refused expected-from` rows (Ruling B44).

  reference.py crosscheck <dumps_dir> <ply_dir>
      draco_decoder's own output (<ply_dir>/<stream>.ply, binary PLY) against
      dump_reference's, byte for byte, wherever the PLY writer can name the
      attribute's type.
"""
import base64
import hashlib
import json
import os
import struct
import sys

# glTF componentType: (struct code, bytes, Draco DataType, Draco's name).
COMPONENT = {
    5120: ("b", 1, 1, "int8"),
    5121: ("B", 1, 2, "uint8"),
    5122: ("h", 2, 3, "int16"),
    5123: ("H", 2, 4, "uint16"),
    5125: ("I", 4, 6, "uint32"),
    5126: ("f", 4, 9, "float32"),
}
DATA_TYPE_NAME = {1: "int8", 2: "uint8", 3: "int16", 4: "uint16", 5: "int32", 6: "uint32", 9: "float32"}
DATA_TYPE_SIZE = {1: 1, 2: 1, 3: 2, 4: 2, 5: 4, 6: 4, 9: 4}
COMPONENTS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}
SEMANTIC_NAME = {0: "POSITION", 1: "NORMAL", 2: "COLOR", 3: "TEX_COORD", 4: "GENERIC"}

# The settings make_reference.sh encodes, in its order: draco_encoder's -cl and
# -qp grid (Ruling B35), then three of our own (encode_reference.cc's usage).
LEVELS = (0, 1, 5, 7, 10)
POSITION_BITS = (8, 11, 14, 16)
SETTINGS = [f"cl{c}-qp{q}" for c in LEVELS for q in POSITION_BITS] + ["seqc-qp11", "swap-qp11", "meta-qp11"]


def draco_type(semantic):
    """The Draco attribute type a glTF Draco encoder gives a semantic."""
    if semantic == "POSITION":
        return 0
    if semantic == "NORMAL":
        return 1
    if semantic.startswith("COLOR_"):
        return 2
    if semantic.startswith("TEXCOORD_"):
        return 3
    return 4  # TANGENT, JOINTS_n, WEIGHTS_n and custom attributes are GENERIC.


def load_gltf(path):
    """The document and its buffers' bytes, from a .glb or a .gltf."""
    data = open(path, "rb").read()
    buffers = []
    if data[:4] == b"glTF":
        at, doc, binary = 12, None, None
        while at < len(data):
            length, kind = struct.unpack_from("<II", data, at)
            chunk = data[at + 8 : at + 8 + length]
            if kind == 0x4E4F534A:
                doc = json.loads(chunk)
            elif kind == 0x004E4942:
                binary = chunk
            at += 8 + length
    else:
        doc, binary = json.loads(data), None
    for b in doc.get("buffers", []):
        uri = b.get("uri")
        if uri is None:
            buffers.append(binary)
        elif uri.startswith("data:"):
            buffers.append(base64.b64decode(uri.split(",", 1)[1]))
        else:
            buffers.append(open(os.path.join(os.path.dirname(path), uri), "rb").read())
    return doc, buffers


def accessor(doc, buffers, index):
    """An accessor's elements, tightly packed: (count, components, Draco type, normalized, bytes)."""
    a = doc["accessors"][index]
    if "sparse" in a or "bufferView" not in a:
        raise SystemExit(f"accessor {index}: sparse or without a bufferView, which the corpus doesn't read")
    _, size, dt, _ = COMPONENT[a["componentType"]]
    n = COMPONENTS[a["type"]]
    view = doc["bufferViews"][a["bufferView"]]
    data = buffers[view["buffer"]]
    stride = view.get("byteStride", n * size)
    start = view.get("byteOffset", 0) + a.get("byteOffset", 0)
    end = view.get("byteOffset", 0) + view["byteLength"]
    out = bytearray()
    for i in range(a["count"]):
        at = start + i * stride
        if at + n * size > end:
            raise SystemExit(f"accessor {index}: element {i} runs past its bufferView")
        out += data[at : at + n * size]
    return a["count"], n, dt, bool(a.get("normalized", False)), bytes(out)


def indices(doc, buffers, prim, points):
    if "indices" not in prim:
        return list(range(points))
    count, n, dt, _, raw = accessor(doc, buffers, prim["indices"])
    code = {2: "B", 4: "H", 6: "I"}[dt]
    return list(struct.unpack(f"<{count}{code}", raw))


def clean(name):
    """A mesh name as part of an input's name, which has no dots: a stream is
    <input>.<setting>.drc."""
    return "".join(c if c.isalnum() or c == "_" else "_" for c in name)


def geometry(out_dir, specs):
    os.makedirs(out_dir, exist_ok=True)
    taken = {}
    rows = []
    for spec in specs:
        prefix, path = spec.split("=", 1)
        doc, buffers = load_gltf(path)
        source_sha = hashlib.sha256(open(path, "rb").read()).hexdigest()
        meshes = doc.get("meshes", [])
        triangles = [(m, p) for m, mesh in enumerate(meshes) for p, prim in enumerate(mesh["primitives"]) if prim.get("mode", 4) == 4]
        for m, p in triangles:
            mesh, prim = meshes[m], meshes[m]["primitives"][p]
            parts = [prefix]
            if len(meshes) > 1:
                parts.append(clean(mesh.get("name") or f"mesh{m}"))
            if len(mesh["primitives"]) > 1:
                parts.append(str(p))
            name = "-".join(parts)
            attributes, points = [], None
            for semantic, index in prim["attributes"].items():
                count, n, dt, normalized, raw = accessor(doc, buffers, index)
                if points is None:
                    points = count
                elif count != points:
                    raise SystemExit(f"{name}: {semantic} has {count} elements, not {points}")
                attributes.append((semantic, draco_type(semantic), dt, n, normalized, raw))
            faces = indices(doc, buffers, prim, points)
            if len(faces) % 3 or any(i >= points for i in faces):
                raise SystemExit(f"{name}: indices aren't whole triangles of its points")
            key = (points, tuple(faces), tuple(a[1:] for a in attributes))
            where = f"{os.path.basename(path)} mesh {m} primitive {p}"
            if key in taken:
                rows.append((name, where, source_sha, points, len(faces) // 3, "", f"same as {taken[key]}"))
                continue
            taken[key] = name
            out = bytearray(b"LGGEOM01")
            out += struct.pack("<III", points, len(faces) // 3, len(attributes))
            out += struct.pack(f"<{len(faces)}I", *faces)
            for semantic, t, dt, n, normalized, raw in attributes:
                label = semantic.encode()
                out += struct.pack("<BBBBB", t, dt, n, int(normalized), len(label)) + label + raw
            open(os.path.join(out_dir, name + ".geom"), "wb").write(bytes(out))
            described = " ".join(f"{s}:{SEMANTIC_NAME[t]}:{DATA_TYPE_NAME[dt]}x{n}{'n' if nz else ''}" for s, t, dt, n, nz, _ in attributes)
            targets = len(prim.get("targets", []))
            rows.append((name, where, source_sha, points, len(faces) // 3, described, f"{targets} morph targets, not encoded" if targets else ""))
    with open(os.path.join(out_dir, "inputs.tsv"), "w") as f:
        f.write("input\tsource\tsource sha256\tpoints\tfaces\tattributes (glTF:Draco:type)\tnote\n")
        for row in rows:
            f.write("\t".join(str(x) for x in row) + "\n")
    print(f"{sum(1 for r in rows if not r[6].startswith('same as'))} inputs, {len(rows)} triangle primitives read")


def varint(data, at):
    value, shift = 0, 0
    while True:
        byte = data[at]
        at += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, at
        shift += 7


def skip_metadata(data, at):
    entries, at = varint(data, at)
    for _ in range(entries):
        at += 1 + data[at]
        size, at = varint(data, at)
        at += size
    subs, at = varint(data, at)
    for _ in range(subs):
        at += 1 + data[at]
        at = skip_metadata(data, at)
    return at


def encoding(data):
    """What the header says: sequential (raw or compressed connectivity), or
    the EdgeBreaker coder; and whether metadata comes first."""
    if data[:5] != b"DRACO" or data[5:7] != b"\x02\x02" or data[7] != 1:
        raise SystemExit("not a Draco 2.2 triangle mesh")
    method, flags, at = data[8], struct.unpack_from("<H", data, 9)[0], 11
    metadata = bool(flags & 0x8000)
    if metadata:
        attributes, at = varint(data, at)
        for _ in range(attributes):
            _, at = varint(data, at)
            at = skip_metadata(data, at)
        at = skip_metadata(data, at)
    if method == 0:
        _, at = varint(data, at)
        _, at = varint(data, at)
        kind = {0: "sequential-compressed", 1: "sequential"}[data[at]]
    else:
        kind = {0: "edgebreaker-standard", 2: "edgebreaker-valence"}[data[at]]
    return kind, metadata


def described(dump):
    """dump_reference's printed description of one stream."""
    points = faces = None
    attributes, metadata = [], None
    for line in open(os.path.join(dump, "description.txt")):
        f = line.split()
        if f[0] == "points":
            points = int(f[1])
        elif f[0] == "faces":
            faces = int(f[1])
        elif f[0] == "attribute":
            index, uid, t, dt, n, nz, stride, offset, values, identity = (int(x) for x in f[1:])
            attributes.append(dict(index=index, uid=uid, type=t, dt=dt, n=n, normalized=nz, stride=stride, offset=offset, values=values, identity=identity))
        elif f[0] == "metadata":
            metadata = f[1]
    return points, faces, attributes, metadata


def geometry_names(path):
    """The glTF semantic of each attribute of a geometry file, by attribute index."""
    data = open(path, "rb").read()
    points, faces, count = struct.unpack_from("<III", data, 8)
    at, names = 20 + 12 * faces, []
    for _ in range(count):
        _, dt, n, _, length = struct.unpack_from("<BBBBB", data, at)
        names.append(data[at + 5 : at + 5 + length].decode())
        at += 5 + length + points * n * DATA_TYPE_SIZE[dt]
    return names


def sha256(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def pair_order(stream):
    """Sort key: input, then the setting in SETTINGS' order."""
    input_name, setting, _ = stream.rsplit(".", 2)
    return input_name, SETTINGS.index(setting)


def store(all_dir, streams_dir, pairs_out):
    streams = sorted((f for f in os.listdir(all_dir) if f.endswith(".drc")), key=pair_order)
    inputs = sorted({f.rsplit(".", 2)[0] for f in streams})
    expected = sorted((f"{i}.{s}.drc" for i in inputs for s in SETTINGS), key=pair_order)
    if streams != expected:
        raise SystemExit(f"streams aren't every input at every setting: {sorted(set(expected) ^ set(streams))}")
    os.makedirs(streams_dir, exist_ok=True)
    stored, rows = {}, []
    for stream in streams:
        data = open(os.path.join(all_dir, stream), "rb").read()
        digest = hashlib.sha256(data).hexdigest()
        if digest not in stored:
            stored[digest] = stream
            open(os.path.join(streams_dir, stream), "wb").write(data)
        rows.append((stream, stored[digest], digest, len(data)))
    with open(pairs_out, "w") as f:
        f.write("pair\tstored as\tsha256\tbytes\n")
        for row in rows:
            f.write("\t".join(str(x) for x in row) + "\n")
    print(f"{len(stored)} distinct streams stored of {len(streams)} pairs ({len(inputs)} inputs, {len(SETTINGS)} settings), {sum(len(open(os.path.join(streams_dir, s), 'rb').read()) for s in stored.values())} bytes")


def read_pairs(path):
    """pairs.tsv: {pair: stored stream}."""
    rows = [line.rstrip("\n").split("\t") for line in open(path)][1:]
    return {pair: stored for pair, stored, _, _ in rows}


def manifest(streams_dir, dumps_dir, geometry_dir, pairs_path, out, inc_out):
    streams = sorted((f for f in os.listdir(streams_dir) if f.endswith(".drc")), key=pair_order)
    pairs = read_pairs(pairs_path)
    inputs = sorted({f.rsplit(".", 2)[0] for f in pairs})
    expected = sorted(f"{i}.{s}.drc" for i in inputs for s in SETTINGS)
    if sorted(pairs) != expected:
        raise SystemExit(f"the pairs aren't every input at every setting: {sorted(set(expected) ^ set(pairs))}")
    if sorted(set(pairs.values())) != sorted(streams):
        raise SystemExit(f"the stored streams aren't the pairs' targets: {sorted(set(pairs.values()) ^ set(streams))}")
    if any(pairs[s] != s for s in streams):
        raise SystemExit("a stored stream isn't its own pair's target")
    lines = [
        "# Google's own decoding of every stream in streams/, the expectations of",
        "# draco_core_matches_googles_decoder (litegen-core/src/mesh/draco.rs).",
        "# Written by make_reference.sh (README.md beside this file): decoded by",
        "# google/draco d429de84b52a860705507da0884764c5d8321423 (1.5.9 and one",
        "# commit), through dump_reference.cc. Don't edit by hand.",
        "#",
        "# stream <file> <points> <faces> <sha256 of the faces' point indices, u32",
        "#   little-endian, three a face, in the decoder's order> <the header's",
        "#   encoding> [metadata]",
        "# attribute <unique id> <Draco semantic> <data type> <components>",
        "#   <normalized> <sha256 of every point's value through the attribute's",
        "#   point map, in point order, little-endian> <the glTF semantic it was>",
        "# same <input>.<setting>.drc <stream file>: that pair's stream is the",
        "#   named stream, byte for byte, so it is stored once (Ruling B42) and",
        "#   decodes to the same expectations.",
        "# A stream row ending `google-refused expected-from <stream file>` is",
        "#   one Google's decoder refuses (the comment above it quotes why,",
        "#   Ruling B44): its counts and hashes are Google's decoding of the named",
        "#   stream, the same input at cl0-qp11, which encodes the same",
        "#   attributes and point order and differs only in its connectivity",
        "#   coding (README.md).",
        "#",
        "# The settings in each stream's name (encode_reference.cc):",
        "#   cl<C>-qp<Q>  draco_encoder -cl C -qp Q, its defaults -qt 10 -qn 8 -qg 8",
        "#   seqc-qp11    -cl 0 -qp 11, with compress_connectivity",
        "#   swap-qp11    -cl 10 -qp 11, with the other EdgeBreaker coder",
        "#   meta-qp11    -cl 7 -qp 11, with metadata",
    ]
    refused = 0
    for stream in streams:
        input_name, setting, _ = stream.rsplit(".", 2)
        dump = os.path.join(dumps_dir, stream[: -len(".drc")])
        data = open(os.path.join(streams_dir, stream), "rb").read()
        kind, has_metadata = encoding(data)
        lines.append("")
        tail = ""
        if os.path.exists(os.path.join(dump, "refused.txt")):
            # Google's sequential decoder refuses a face count past the
            # remaining bytes / 3 (mesh_sequential_decoder.cc:58), which
            # compressed connectivity can beat. Its decoding of the same input
            # at cl0-qp11 is the expectation: the same attributes, point
            # order and quantization, raw connectivity.
            if setting != "seqc-qp11" or kind != "sequential-compressed":
                raise SystemExit(f"{stream}: Google's decoder refused a stream that isn't compressed sequential connectivity")
            substitute = pairs[f"{input_name}.cl0-qp11.drc"]
            why = open(os.path.join(dump, "refused.txt")).read().strip().replace("dump_reference: decoding failed: ", "")
            lines.append(f"# Google's decoder refuses {stream} ({why}): the row is its decoding of {substitute}.")
            dump = os.path.join(dumps_dir, substitute[: -len(".drc")])
            tail = f" google-refused expected-from {substitute}"
            refused += 1
        points, faces, attributes, metadata = described(dump)
        if has_metadata != (metadata is not None):
            raise SystemExit(f"{stream}: the header's metadata flag and Google's decoding disagree")
        names = geometry_names(os.path.join(geometry_dir, input_name + ".geom"))
        lines.append(f"stream {stream} {points} {faces} {sha256(os.path.join(dump, 'connectivity.bin'))} {kind}" + (" metadata" if has_metadata else "") + tail)
        for a in sorted(attributes, key=lambda a: a["uid"]):
            width = a["n"] * DATA_TYPE_SIZE[a["dt"]]
            if a["stride"] != width or a["offset"] != 0:
                raise SystemExit(f"{stream}: attribute {a['uid']} isn't packed ({a['stride']}, {a['offset']})")
            path = os.path.join(dump, f"attribute-{a['uid']}.bin")
            if os.path.getsize(path) != points * width:
                raise SystemExit(f"{stream}: attribute {a['uid']}'s dump isn't a value a point")
            lines.append(
                f"attribute {a['uid']} {SEMANTIC_NAME[a['type']]} {DATA_TYPE_NAME[a['dt']]} {a['n']} {a['normalized']} {sha256(path)} {names[a['uid']]}"
            )
        for pair in sorted((p for p, s in pairs.items() if s == stream and p != stream), key=pair_order):
            lines.append(f"same {pair} {stream}")
    with open(out, "w") as f:
        f.write("\n".join(lines) + "\n")
    with open(inc_out, "w") as f:
        f.write("// Every stream in streams/, by name, for draco_core_matches_googles_decoder\n")
        f.write("// (litegen-core/src/mesh/draco.rs), which include!s this listing beside\n")
        f.write("// manifest.txt. Written by make_reference.sh (reference.py). Don't edit.\n")
        f.write("&[\n")
        for stream in streams:
            f.write(f'    ("{stream}", include_bytes!("streams/{stream}")),\n')
        f.write("]\n")
    print(f"{len(streams)} streams stored, {len(pairs)} pairs, {len(inputs)} inputs, {len(SETTINGS)} settings; Google's decoder refused {refused}")


def equivalence(dumps_dir, pairs_path):
    pairs = read_pairs(pairs_path)
    inputs = sorted({f.rsplit(".", 2)[0] for f in pairs})
    equal, refused = 0, []
    for input_name in inputs:
        seqc = os.path.join(dumps_dir, pairs[f"{input_name}.seqc-qp11.drc"][: -len(".drc")])
        raw = os.path.join(dumps_dir, pairs[f"{input_name}.cl0-qp11.drc"][: -len(".drc")])
        if os.path.exists(os.path.join(raw, "refused.txt")):
            raise SystemExit(f"{input_name}: Google's decoder refused the cl0-qp11 stream, so there is no substitute")
        if os.path.exists(os.path.join(seqc, "refused.txt")):
            refused.append(input_name)
            continue
        files = sorted(f for f in os.listdir(raw) if f.endswith(".bin"))
        if files != sorted(f for f in os.listdir(seqc) if f.endswith(".bin")):
            raise SystemExit(f"{input_name}: seqc-qp11 and cl0-qp11 decode to different attributes")
        for f in files:
            if open(os.path.join(seqc, f), "rb").read() != open(os.path.join(raw, f), "rb").read():
                raise SystemExit(f"{input_name}: {f} differs between seqc-qp11 and cl0-qp11")
        if described(seqc)[:3] != described(raw)[:3]:
            raise SystemExit(f"{input_name}: the descriptions differ between seqc-qp11 and cl0-qp11")
        equal += 1
    print(f"{equal} of {len(inputs)} inputs: Google's decoding of seqc-qp11 equals its decoding of cl0-qp11, byte for byte; refused seqc-qp11: {', '.join(refused) or 'none'}")


PLY_TYPES = {"float": ("f", 4), "uchar": ("B", 1), "int": ("i", 4)}


def read_ply(path):
    """draco_decoder's binary PLY: its vertex rows and its face rows, as bytes."""
    data = open(path, "rb").read()
    end = data.index(b"end_header\n") + len(b"end_header\n")
    header = data[:end].decode().splitlines()
    if "format binary_little_endian 1.0" not in header:
        raise SystemExit(f"{path}: not binary little-endian")
    vertex_props, face_lists, element = [], [], None
    for line in header:
        f = line.split()
        if f[0] == "element":
            element, count = f[1], int(f[2])
            if element == "vertex":
                vertices = count
            else:
                faces = count
        elif f[0] == "property" and element == "vertex":
            vertex_props.append((f[2], f[1]))
        elif f[0] == "property" and element == "face":
            face_lists.append((f[4], f[3]))
    at = end
    width = sum(PLY_TYPES[t][1] for _, t in vertex_props)
    rows = [data[at + i * width : at + (i + 1) * width] for i in range(vertices)]
    at += vertices * width
    face_rows = []
    for _ in range(faces):
        row = {}
        for name, t in face_lists:
            n = data[at]
            size = PLY_TYPES[t][1]
            row[name] = data[at + 1 : at + 1 + n * size]
            at += 1 + n * size
        face_rows.append(row)
    if at != len(data):
        raise SystemExit(f"{path}: {len(data) - at} bytes after the faces")
    return vertex_props, rows, face_lists, face_rows


def crosscheck(dumps_dir, ply_dir):
    checked, skipped = 0, []
    for stream in sorted(os.listdir(dumps_dir)):
        dump = os.path.join(dumps_dir, stream)
        if os.path.exists(os.path.join(dump, "refused.txt")):
            skipped.append((stream, "Google's decoder refuses the stream (manifest.txt)"))
            continue
        points, faces, attributes, _ = described(dump)
        first = {}
        for a in sorted(attributes, key=lambda a: a["index"]):
            first.setdefault(a["type"], a)
        # The PLY writer's attributes: the first POSITION, NORMAL (if it has 3
        # components), COLOR, and TEX_COORD (if it has 2) by attribute index,
        # and it names float, uchar and int alone (ply_encoder.cc).
        used = [first[0]] + [first[t] for t in (1, 2, 3) if t in first and (t not in (1, 3) or first[t]["n"] == {1: 3, 3: 2}[t])]
        unnamed = [a for a in used if a["dt"] not in (2, 5, 9)]
        if unnamed:
            skipped.append((stream, ", ".join(f"{SEMANTIC_NAME[a['type']]} {DATA_TYPE_NAME[a['dt']]}" for a in unnamed)))
            continue
        props, rows, _, face_rows = read_ply(os.path.join(ply_dir, stream + ".ply"))
        if len(rows) != points or len(face_rows) != faces:
            raise SystemExit(f"{stream}: draco_decoder wrote {len(rows)} points and {len(face_rows)} faces, the dump {points} and {faces}")
        values = {t: open(os.path.join(dump, f"attribute-{first[t]['uid']}.bin"), "rb").read() for t in first}
        widths = {t: first[t]["n"] * DATA_TYPE_SIZE[first[t]["dt"]] for t in first}
        vertex_types = [t for t in (0, 1, 2) if first.get(t) in used]
        for p, row in enumerate(rows):
            expected = b"".join(values[t][p * widths[t] : (p + 1) * widths[t]] for t in vertex_types)
            if row != expected:
                raise SystemExit(f"{stream}: point {p} differs between draco_decoder and dump_reference")
        connectivity = open(os.path.join(dump, "connectivity.bin"), "rb").read()
        for f, row in enumerate(face_rows):
            corners = struct.unpack("<3i", row["vertex_indices"])
            if struct.pack("<3I", *corners) != connectivity[12 * f : 12 * f + 12]:
                raise SystemExit(f"{stream}: face {f} differs between draco_decoder and dump_reference")
            if 3 in first and first[3] in used:
                w = widths[3]
                expected = b"".join(values[3][c * w : (c + 1) * w] for c in corners)
                if row["texcoord"] != expected:
                    raise SystemExit(f"{stream}: face {f}'s texture coordinates differ")
        checked += 1
    print(f"{checked} streams equal draco_decoder's PLY, byte for byte; {len(skipped)} not checked:")
    for stream, why in skipped:
        print(f"  {stream}: {why}" if why.startswith("Google") else f"  {stream}: draco_decoder's PLY writer names no type for {why}")


if __name__ == "__main__":
    command, args = sys.argv[1], sys.argv[2:]
    if command == "geometry":
        geometry(args[0], args[1:])
    elif command == "store":
        store(*args)
    elif command == "manifest":
        manifest(*args)
    elif command == "equivalence":
        equivalence(*args)
    elif command == "crosscheck":
        crosscheck(*args)
    else:
        raise SystemExit(__doc__)
