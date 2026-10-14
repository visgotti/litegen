#!/usr/bin/env python3
"""Tiny from-scratch binary FBX writer used to probe what readers require.

Usage: minifbx.py OUT_DIR
Writes several variants of the same small scene (a skinned, animated quad with
one material) so they can be loaded by assimp and three.js.
"""
import os, struct, sys, zlib

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)

ALWAYS_SENTINEL = {"AnimationLayer", "AnimationStack"}
FOOTER_ID = bytes.fromhex("fabcab09d0c8d466b176fb831cf7267e")
FOOTER_MAGIC = bytes.fromhex("f85a8c6adef5d97eece90ce3758f290b")
FILE_ID = bytes.fromhex("28b32aebb624ccc2bfc8b02aa92bfcf1")
CTIME = "1970-01-01 10:00:00:000"


class N:
    def __init__(self, name, *props, children=None):
        self.name, self.props, self.children = name, list(props), list(children or [])

    def add(self, *kids):
        self.children.extend(kids)
        return self


# property helpers -> (typecode, value)
def I(v): return ("I", v)
def L(v): return ("L", v)
def D(v): return ("D", v)
def S(v): return ("S", v.encode() if isinstance(v, str) else v)
def C(v): return ("C", v)
def R(v): return ("R", v)
def ad(v): return ("d", list(v))
def ai(v): return ("i", list(v))
def al(v): return ("l", list(v))
def af(v): return ("f", list(v))


def name_class(name, cls):  # binary "Name\x00\x01Class"
    return S(name.encode() + b"\x00\x01" + cls.encode())


def enc_prop(t, v, compress):
    if t == "C": return b"C" + bytes([1 if v else 0])
    if t == "Y": return b"Y" + struct.pack("<h", v)
    if t == "I": return b"I" + struct.pack("<i", v)
    if t == "F": return b"F" + struct.pack("<f", v)
    if t == "D": return b"D" + struct.pack("<d", v)
    if t == "L": return b"L" + struct.pack("<q", v)
    if t in "SR": return t.encode() + struct.pack("<I", len(v)) + v
    fmt = {"d": "d", "f": "f", "i": "i", "l": "q", "b": "B"}[t]
    raw = struct.pack("<%d%s" % (len(v), fmt), *v)
    if compress and len(raw) >= 128:
        z = zlib.compress(raw)
        return t.encode() + struct.pack("<III", len(v), 1, len(z)) + z
    return t.encode() + struct.pack("<III", len(v), 0, len(raw)) + raw


def P(name, typ, label, flags, *vals):
    """Properties70 'P' record: name, type, label, flags, values..."""
    out = [S(name), S(typ), S(label), S(flags)]
    for v in vals:
        if isinstance(v, str): out.append(S(v))
        elif typ in ("int", "enum", "Integer", "bool") and isinstance(v, int): out.append(I(v))
        elif typ in ("KTime", "ULongLong"): out.append(L(v))
        else: out.append(D(float(v)))
    return N("P", *out)


def write(path, roots, version=7400, footer=True, compress=True, sentinel_rule=True):
    wide = version >= 7500
    hdr = 25 if wide else 13
    buf = bytearray(b"Kaydara FBX Binary  \x00\x1a\x00" + struct.pack("<I", version))

    def emit(n):
        start = len(buf)
        buf.extend(b"\x00" * hdr)
        nm = n.name.encode()
        buf[start + hdr - 1] = len(nm)
        buf.extend(nm)
        p0 = len(buf)
        for t, v in n.props:
            buf.extend(enc_prop(t, v, compress))
        plen = len(buf) - p0
        for c in n.children:
            emit(c)
        need_null = bool(n.children) or not n.props or (sentinel_rule and n.name in ALWAYS_SENTINEL)
        if need_null:
            buf.extend(b"\x00" * hdr)
        end = len(buf)
        fmt = "<QQQ" if wide else "<III"
        buf[start:start + hdr - 1] = struct.pack(fmt, end, len(n.props), plen)

    for r in roots:
        emit(r)
    buf.extend(b"\x00" * hdr)  # top-level NULL record
    if footer:
        buf.extend(FOOTER_ID)
        pad = 16 - (len(buf) % 16)  # 1..16, SDK files land the next field on a 16 boundary
        buf.extend(b"\x00" * pad)
        buf.extend(b"\x00" * 4)
        buf.extend(struct.pack("<I", version))
        buf.extend(b"\x00" * 120)
        buf.extend(FOOTER_MAGIC)
    with open(path, "wb") as f:
        f.write(buf)


# ---------------------------------------------------------------- scene
TPS = 46186158000  # FBX ticks per second


def scene(by_ctrl_point="ByVertice", uv_mapping="ByPolygonVertex"):
    # quad 0..1 in X/Y, one n-gon polygon, 2 bones along +Y
    verts = [0, 0, 0, 1, 0, 0, 1, 2, 0, 0, 2, 0]
    pvi = [0, 1, 2, ~3]
    geo = N("Geometry", L(1001), name_class("Quad", "Geometry"), S("Mesh")).add(
        N("Properties70"),
        N("GeometryVersion", I(124)),
        N("Vertices", ad(verts)),
        N("PolygonVertexIndex", ai(pvi)),
        N("LayerElementNormal", I(0)).add(
            N("Version", I(101)), N("Name", S("")),
            N("MappingInformationType", S(by_ctrl_point)),
            N("ReferenceInformationType", S("Direct")),
            N("Normals", ad([0, 0, 1] * 4))),
        N("LayerElementUV", I(0)).add(
            N("Version", I(101)), N("Name", S("UVMap")),
            N("MappingInformationType", S(uv_mapping)),
            N("ReferenceInformationType", S("IndexToDirect")),
            N("UV", ad([0, 0, 1, 0, 1, 1, 0, 1])),
            N("UVIndex", ai([0, 1, 2, 3]))),
        N("LayerElementMaterial", I(0)).add(
            N("Version", I(101)), N("Name", S("")),
            N("MappingInformationType", S("AllSame")),
            N("ReferenceInformationType", S("IndexToDirect")),
            N("Materials", ai([0]))),
        N("Layer", I(0)).add(
            N("Version", I(100)),
            N("LayerElement").add(N("Type", S("LayerElementNormal")), N("TypedIndex", I(0))),
            N("LayerElement").add(N("Type", S("LayerElementUV")), N("TypedIndex", I(0))),
            N("LayerElement").add(N("Type", S("LayerElementMaterial")), N("TypedIndex", I(0)))),
    )

    def model(i, name, cls, t=(0, 0, 0), r=(0, 0, 0)):
        return N("Model", L(i), name_class(name, "Model"), S(cls)).add(
            N("Version", I(232)),
            N("Properties70").add(
                P("RotationActive", "bool", "", "", 1),
                P("InheritType", "enum", "", "", 1),
                P("DefaultAttributeIndex", "int", "Integer", "", 0),
                P("Lcl Translation", "Lcl Translation", "", "A", *t),
                P("Lcl Rotation", "Lcl Rotation", "", "A", *r),
                P("Lcl Scaling", "Lcl Scaling", "", "A", 1, 1, 1)),
            N("Shading", C(True)), N("Culling", S("CullingOff")))

    mesh_m = model(2001, "QuadMesh", "Mesh")
    b0 = model(2002, "Bone0", "LimbNode")
    b1 = model(2003, "Bone1", "LimbNode", t=(0, 1, 0))
    att0 = N("NodeAttribute", L(2102), name_class("Bone0", "NodeAttribute"), S("LimbNode")).add(N("TypeFlags", S("Skeleton")))
    att1 = N("NodeAttribute", L(2103), name_class("Bone1", "NodeAttribute"), S("LimbNode")).add(N("TypeFlags", S("Skeleton")))
    mat = N("Material", L(3001), name_class("Mat", "Material"), S("")).add(
        N("Version", I(102)), N("ShadingModel", S("phong")), N("MultiLayer", I(0)),
        N("Properties70").add(P("DiffuseColor", "Color", "", "A", 0.8, 0.2, 0.1), P("Opacity", "double", "Number", "", 1.0)))
    ident = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
    b1_bind = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 0, 1]
    b1_inv = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1]
    skin = N("Deformer", L(4001), name_class("Skin", "Deformer"), S("Skin")).add(
        N("Version", I(101)), N("Link_DeformAcuracy", D(50.0)), N("SkinningType", S("Linear")))
    cl0 = N("Deformer", L(4002), name_class("Bone0", "SubDeformer"), S("Cluster")).add(
        N("Version", I(100)), N("UserData", S(""), S("")),
        N("Indexes", ai([0, 1])), N("Weights", ad([1, 1])),
        N("Transform", ad(ident)), N("TransformLink", ad(ident)))
    cl1 = N("Deformer", L(4003), name_class("Bone1", "SubDeformer"), S("Cluster")).add(
        N("Version", I(100)), N("UserData", S(""), S("")),
        N("Indexes", ai([2, 3])), N("Weights", ad([1, 1])),
        N("Transform", ad(b1_inv)), N("TransformLink", ad(b1_bind)))
    pose = N("Pose", L(5001), name_class("BindPose", "Pose"), S("BindPose")).add(
        N("Type", S("BindPose")), N("Version", I(100)), N("NbPoseNodes", I(3)),
        N("PoseNode").add(N("Node", L(2001)), N("Matrix", ad(ident))),
        N("PoseNode").add(N("Node", L(2002)), N("Matrix", ad(ident))),
        N("PoseNode").add(N("Node", L(2003)), N("Matrix", ad(b1_bind))))
    # animation: Bone1 rotates about Z 0 -> 90 deg over 1 s, 3 keys, linear
    stack = N("AnimationStack", L(6001), name_class("Wave", "AnimStack"), S("")).add(
        N("Properties70").add(P("LocalStop", "KTime", "Time", "", TPS), P("ReferenceStop", "KTime", "Time", "", TPS)))
    layer = N("AnimationLayer", L(6002), name_class("BaseLayer", "AnimLayer"), S(""))
    cn = N("AnimationCurveNode", L(6003), name_class("R", "AnimCurveNode"), S("")).add(
        N("Properties70").add(P("d|X", "Number", "", "A", 0.0), P("d|Y", "Number", "", "A", 0.0), P("d|Z", "Number", "", "A", 0.0)))
    weights = struct.unpack("<f", struct.pack("<I", 0x0D050D05))[0]

    def curve(i, vals):
        return N("AnimationCurve", L(i), name_class("", "AnimCurve"), S("")).add(
            N("Default", D(vals[0])), N("KeyVer", I(4009)),
            N("KeyTime", al([0, TPS // 2, TPS])), N("KeyValueFloat", af(vals)),
            N("KeyAttrFlags", ai([0x4 | 0x100 | 0x2000])),  # linear (tangent bits as written by SDK)
            N("KeyAttrDataFloat", af([0, 0, weights, 0])), N("KeyAttrRefCount", ai([3])))

    cx, cy, cz = curve(6011, [0, 0, 0]), curve(6012, [0, 0, 0]), curve(6013, [0, 45, 90])
    objects = N("Objects").add(geo, mesh_m, b0, b1, att0, att1, mat, skin, cl0, cl1, pose, stack, layer, cn, cx, cy, cz)
    C_ = lambda *p: N("C", *p)
    conns = N("Connections").add(
        C_(S("OO"), L(2001), L(0)), C_(S("OO"), L(1001), L(2001)), C_(S("OO"), L(3001), L(2001)),
        C_(S("OO"), L(2002), L(0)), C_(S("OO"), L(2003), L(2002)),
        C_(S("OO"), L(2102), L(2002)), C_(S("OO"), L(2103), L(2003)),
        C_(S("OO"), L(4001), L(1001)), C_(S("OO"), L(4002), L(4001)), C_(S("OO"), L(4003), L(4001)),
        C_(S("OO"), L(2002), L(4002)), C_(S("OO"), L(2003), L(4003)),
        C_(S("OO"), L(6002), L(6001)), C_(S("OO"), L(6003), L(6002)),
        C_(S("OP"), L(6003), L(2003), S("Lcl Rotation")),
        C_(S("OP"), L(6011), L(6003), S("d|X")), C_(S("OP"), L(6012), L(6003), S("d|Y")), C_(S("OP"), L(6013), L(6003), S("d|Z")))
    return objects, conns


def header(version):
    return N("FBXHeaderExtension").add(
        N("FBXHeaderVersion", I(1003)), N("FBXVersion", I(version)), N("EncryptionType", I(0)),
        N("CreationTimeStamp").add(N("Version", I(1000)), N("Year", I(2026)), N("Month", I(1)), N("Day", I(1)),
                                   N("Hour", I(0)), N("Minute", I(0)), N("Second", I(0)), N("Millisecond", I(0))),
        N("Creator", S("probe-writer 0.1")))


def gsettings(unit=100.0):
    return N("GlobalSettings").add(N("Version", I(1000)), N("Properties70").add(
        P("UpAxis", "int", "Integer", "", 1), P("UpAxisSign", "int", "Integer", "", 1),
        P("FrontAxis", "int", "Integer", "", 2), P("FrontAxisSign", "int", "Integer", "", 1),
        P("CoordAxis", "int", "Integer", "", 0), P("CoordAxisSign", "int", "Integer", "", 1),
        P("OriginalUpAxis", "int", "Integer", "", 1), P("OriginalUpAxisSign", "int", "Integer", "", 1),
        P("UnitScaleFactor", "double", "Number", "", unit), P("OriginalUnitScaleFactor", "double", "Number", "", unit),
        P("TimeMode", "enum", "", "", 6), P("TimeSpanStart", "KTime", "Time", "", 0),
        P("TimeSpanStop", "KTime", "Time", "", TPS), P("CustomFrameRate", "double", "Number", "", -1)))


def definitions():
    def ot(name, count):
        return N("ObjectType", S(name)).add(N("Count", I(count)))
    return N("Definitions").add(N("Version", I(100)), N("Count", I(18)),
                                ot("GlobalSettings", 1), ot("Geometry", 1), ot("Model", 3), ot("NodeAttribute", 2),
                                ot("Material", 1), ot("Deformer", 3), ot("Pose", 1), ot("AnimationStack", 1),
                                ot("AnimationLayer", 1), ot("AnimationCurveNode", 1), ot("AnimationCurve", 3))


def full(version, **kw):
    objects, conns = scene(**kw)
    return [header(version), N("FileId", R(FILE_ID)), N("CreationTime", S(CTIME)), N("Creator", S("probe-writer 0.1")),
            gsettings(),
            N("Documents").add(N("Count", I(1)), N("Document", L(9999), S("Scene"), S("Scene")).add(
                N("Properties70").add(P("SourceObject", "object", "", ""), P("ActiveAnimStackName", "KString", "", "", "Wave")),
                N("RootNode", L(0)))),
            N("References"), definitions(), objects, conns,
            N("Takes").add(N("Current", S("Wave")), N("Take", S("Wave")).add(
                N("FileName", S("Wave.tak")), N("LocalTime", L(0), L(TPS)), N("ReferenceTime", L(0), L(TPS)))) ]


for ver in (7400, 7500):
    write(os.path.join(OUT, "full_%d.fbx" % ver), full(ver), version=ver)
    write(os.path.join(OUT, "nofooter_%d.fbx" % ver), full(ver), version=ver, footer=False)
    objects, conns = scene()
    write(os.path.join(OUT, "bare_%d.fbx" % ver), [objects, conns], version=ver)
    write(os.path.join(OUT, "hdr_gs_obj_%d.fbx" % ver), [header(ver), gsettings(), objects, conns], version=ver)
    r = full(ver)
    write(os.path.join(OUT, "nodefs_%d.fbx" % ver), [n for n in r if n.name != "Definitions"], version=ver)
    write(os.path.join(OUT, "uncompressed_%d.fbx" % ver), full(ver), version=ver, compress=False)
    write(os.path.join(OUT, "bycontrolpoint_%d.fbx" % ver), full(ver, by_ctrl_point="ByControlPoint", uv_mapping="ByControlPoint"), version=ver)
    write(os.path.join(OUT, "nosentinel_%d.fbx" % ver), full(ver), version=ver, sentinel_rule=False)
print("ok")
