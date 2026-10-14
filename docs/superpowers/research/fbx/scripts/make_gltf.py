#!/usr/bin/env python3
"""Generate small glTF/GLB test scenes to feed `assimp export ... .fbx`.

Usage: make_gltf.py OUT_DIR
Writes: scene.gltf + scene.bin + *.png, and scene.glb (images embedded).
"""
import json, math, os, struct, sys, zlib

out = sys.argv[1]
os.makedirs(out, exist_ok=True)


def png(w, h, rgba_fn):
    raw = b""
    for y in range(h):
        raw += b"\x00"
        for x in range(w):
            raw += bytes(rgba_fn(x, y))

    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


images = {
    "base.png": png(4, 4, lambda x, y: (255 if (x + y) % 2 else 40, 80, 40, 255)),
    "normal.png": png(4, 4, lambda x, y: (128, 128, 255, 255)),
    "mr.png": png(4, 4, lambda x, y: (0, 100, 200, 255)),  # G=roughness B=metallic
    "emissive.png": png(4, 4, lambda x, y: (255, 255, 0, 255)),
    "ao.png": png(4, 4, lambda x, y: (200, 0, 0, 255)),
}

buf = bytearray()
views, accessors = [], []


def add(data_bytes, target=None):
    while len(buf) % 4:
        buf.append(0)
    off = len(buf)
    buf.extend(data_bytes)
    v = {"buffer": 0, "byteOffset": off, "byteLength": len(data_bytes)}
    if target:
        v["target"] = target
    views.append(v)
    return len(views) - 1


def acc(values, comp, typ, count, target=None, minmax=False, normalized=False):
    fmt = {5126: "f", 5123: "H", 5121: "B", 5125: "I"}[comp]
    data = struct.pack("<%d%s" % (len(values), fmt), *values)
    a = {"bufferView": add(data, target), "componentType": comp, "type": typ, "count": count}
    if normalized:
        a["normalized"] = True
    if minmax:
        n = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[typ]
        a["min"] = [min(values[i::n]) for i in range(n)]
        a["max"] = [max(values[i::n]) for i in range(n)]
    accessors.append(a)
    return len(accessors) - 1


# ---- cube (24 verts) -------------------------------------------------------
faces = [  # normal, u axis, v axis
    ((1, 0, 0), (0, 0, -1), (0, 1, 0)), ((-1, 0, 0), (0, 0, 1), (0, 1, 0)),
    ((0, 1, 0), (1, 0, 0), (0, 0, -1)), ((0, -1, 0), (1, 0, 0), (0, 0, 1)),
    ((0, 0, 1), (1, 0, 0), (0, 1, 0)), ((0, 0, -1), (-1, 0, 0), (0, 1, 0)),
]
P, N, UV, C, IDX = [], [], [], [], []
for fi, (n, u, v) in enumerate(faces):
    base = len(P) // 3
    for (su, sv), (tu, tv) in zip([(-1, -1), (1, -1), (1, 1), (-1, 1)], [(0, 1), (1, 1), (1, 0), (0, 0)]):
        p = [0.5 * (n[k] + su * u[k] + sv * v[k]) for k in range(3)]
        P += p; N += list(n); UV += [tu, tv]; C += [fi / 5.0, 0.5, 1 - fi / 5.0, 1.0]
    IDX += [base, base + 1, base + 2, base, base + 2, base + 3]

cube_p = acc(P, 5126, "VEC3", 24, 34962, minmax=True)
cube_n = acc(N, 5126, "VEC3", 24, 34962)
cube_uv = acc(UV, 5126, "VEC2", 24, 34962)
cube_c = acc(C, 5126, "VEC4", 24, 34962)
cube_i = acc(IDX, 5123, "SCALAR", 36, 34963)

# ---- skinned strip (6 verts, 2 joints) -------------------------------------
SP = [-0.25, 0, 0, 0.25, 0, 0, -0.25, 1, 0, 0.25, 1, 0, -0.25, 2, 0, 0.25, 2, 0]
SN = [0, 0, 1] * 6
SJ = [0, 1, 0, 0] * 6
SW = [1, 0, 0, 0, 1, 0, 0, 0, 0.5, 0.5, 0, 0, 0.5, 0.5, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0]
SI = [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4]
s_p = acc(SP, 5126, "VEC3", 6, 34962, minmax=True)
s_n = acc(SN, 5126, "VEC3", 6, 34962)
s_j = acc(SJ, 5121, "VEC4", 6, 34962)
s_w = acc(SW, 5126, "VEC4", 6, 34962)
s_i = acc(SI, 5123, "SCALAR", 12, 34963)
ibm = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1,
       1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1]  # column-major; J1 bind at y=+1
s_ibm = acc(ibm, 5126, "MAT4", 2)

# ---- morph quad (4 verts, 2 targets) ---------------------------------------
MP = [0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0]
MN = [0, 0, 1] * 4
MI = [0, 1, 2, 0, 2, 3]
T0 = [0, 0, 0, 0, 0, 0, 0, 0, 0.5, 0, 0, 0.5]       # lift top edge in +Z
T1 = [0, 0, 0, 0.3, 0, 0, 0.3, 0, 0, 0, 0, 0]       # widen right edge in +X
T1N = [0, 0, 0, 0.1, 0, -0.1, 0.1, 0, -0.1, 0, 0, 0]
m_p = acc(MP, 5126, "VEC3", 4, 34962, minmax=True)
m_n = acc(MN, 5126, "VEC3", 4, 34962)
m_i = acc(MI, 5123, "SCALAR", 6, 34963)
m_t0 = acc(T0, 5126, "VEC3", 4, 34962, minmax=True)
m_t1 = acc(T1, 5126, "VEC3", 4, 34962, minmax=True)
m_t1n = acc(T1N, 5126, "VEC3", 4, 34962)

# ---- animation -------------------------------------------------------------
def qz(deg):
    r = math.radians(deg) / 2
    return [0, 0, math.sin(r), math.cos(r)]

t_rot = acc([0, 0.5, 1.0], 5126, "SCALAR", 3, minmax=True)
v_rot = acc(qz(0) + qz(45) + qz(90), 5126, "VEC4", 3)
t_tr = acc([0, 1.0], 5126, "SCALAR", 2, minmax=True)
v_tr = acc([0, 0, 0, 0, 0.5, 0], 5126, "VEC3", 2)
t_sc = acc([0, 1.0], 5126, "SCALAR", 2, minmax=True)
v_sc = acc([1, 1, 1, 2, 2, 2], 5126, "VEC3", 2)
t_mw = acc([0, 1.0], 5126, "SCALAR", 2, minmax=True)
v_mw = acc([0, 0, 1, 0.5], 5126, "SCALAR", 4)

gltf = {
    "asset": {"version": "2.0", "generator": "fbx-research-test"},
    "scene": 0,
    "scenes": [{"nodes": [0, 2, 5]}],
    "nodes": [
        {"name": "Root", "translation": [1, 2, 3], "rotation": [0, math.sin(math.pi / 4), 0, math.cos(math.pi / 4)],
         "scale": [2, 2, 2], "children": [1]},
        {"name": "CubeNode", "mesh": 0, "translation": [0, 0.5, 0]},
        {"name": "Armature", "children": [3, 4]},
        {"name": "J0", "children": [6]},
        {"name": "SkinnedStrip", "mesh": 1, "skin": 0},
        {"name": "MorphNode", "mesh": 2, "translation": [-3, 0, 0]},
        {"name": "J1", "translation": [0, 1, 0]},
    ],
    "meshes": [
        {"name": "CubeMesh", "primitives": [{"attributes": {"POSITION": cube_p, "NORMAL": cube_n, "TEXCOORD_0": cube_uv,
                                                             "COLOR_0": cube_c}, "indices": cube_i, "material": 0}]},
        {"name": "StripMesh", "primitives": [{"attributes": {"POSITION": s_p, "NORMAL": s_n, "JOINTS_0": s_j,
                                                              "WEIGHTS_0": s_w}, "indices": s_i, "material": 1}]},
        {"name": "MorphMesh", "weights": [0.25, 0.0], "extras": {"targetNames": ["Lift", "Widen"]},
         "primitives": [{"attributes": {"POSITION": m_p, "NORMAL": m_n}, "indices": m_i, "material": 1,
                         "targets": [{"POSITION": m_t0}, {"POSITION": m_t1, "NORMAL": m_t1n}]}]},
    ],
    "skins": [{"name": "StripSkin", "joints": [3, 6], "inverseBindMatrices": s_ibm, "skeleton": 3}],
    "materials": [
        {"name": "MatPBR",
         "pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.2, 0.1, 0.5], "metallicFactor": 0.7,
                                  "roughnessFactor": 0.3, "baseColorTexture": {"index": 0},
                                  "metallicRoughnessTexture": {"index": 2}},
         "normalTexture": {"index": 1, "scale": 1.0}, "occlusionTexture": {"index": 4},
         "emissiveTexture": {"index": 3}, "emissiveFactor": [0.1, 0.2, 0.3], "alphaMode": "BLEND",
         "doubleSided": True},
        {"name": "MatPlain", "pbrMetallicRoughness": {"baseColorFactor": [0.2, 0.6, 0.9, 1.0], "metallicFactor": 0.0,
                                                     "roughnessFactor": 0.9}},
    ],
    "textures": [{"source": i} for i in range(5)],
    "animations": [{"name": "Wave", "samplers": [
        {"input": t_rot, "output": v_rot, "interpolation": "LINEAR"},
        {"input": t_tr, "output": v_tr, "interpolation": "STEP"},
        {"input": t_sc, "output": v_sc, "interpolation": "LINEAR"},
        {"input": t_mw, "output": v_mw, "interpolation": "LINEAR"},
    ], "channels": [
        {"sampler": 0, "target": {"node": 6, "path": "rotation"}},
        {"sampler": 1, "target": {"node": 3, "path": "translation"}},
        {"sampler": 2, "target": {"node": 1, "path": "scale"}},
        {"sampler": 3, "target": {"node": 5, "path": "weights"}},
    ]}],
}

names = list(images)
# .gltf with external files
g1 = json.loads(json.dumps(gltf))
g1["images"] = [{"uri": n} for n in names]
g1["buffers"] = [{"uri": "scene.bin", "byteLength": len(buf)}]
g1["bufferViews"] = views
g1["accessors"] = accessors
with open(os.path.join(out, "scene.gltf"), "w") as f:
    json.dump(g1, f, indent=1)
with open(os.path.join(out, "scene.bin"), "wb") as f:
    f.write(buf)
for n, d in images.items():
    with open(os.path.join(out, n), "wb") as f:
        f.write(d)

# .glb with embedded images
g2 = json.loads(json.dumps(gltf))
views2 = list(views)
img_entries = []
for n in names:
    vi = add(images[n])
    img_entries.append({"bufferView": vi, "mimeType": "image/png", "name": n.split(".")[0]})
views2 = views
g2["images"] = img_entries
while len(buf) % 4:
    buf.append(0)
g2["buffers"] = [{"byteLength": len(buf)}]
g2["bufferViews"] = views2
g2["accessors"] = accessors
js = json.dumps(g2).encode()
while len(js) % 4:
    js += b" "
glb = struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(buf))
glb += struct.pack("<II", len(js), 0x4E4F534A) + js + struct.pack("<II", len(buf), 0x004E4942) + bytes(buf)
with open(os.path.join(out, "scene.glb"), "wb") as f:
    f.write(glb)
print("ok", len(buf))
