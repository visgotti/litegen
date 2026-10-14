#!/usr/bin/env python3
"""Generate a small but feature-rich glTF 2.0 test asset (stdlib only).

Features: node hierarchy, 2 materials (textured PBR + plain), base colour /
metallicRoughness / normal textures (PNG), COLOR_0, skin (2 joints + IBMs),
one morph target, animations (rotation LINEAR, translation LINEAR,
scale STEP, morph weights LINEAR).

Base colour texture quadrants (IMAGE space, row 0 = top):
  top-left RED, top-right GREEN, bottom-left BLUE, bottom-right WHITE.
The strip is UV-mapped so that, viewed from +Z, the image appears upright.
"""
import json, math, os, struct, sys, zlib

out = sys.argv[1]
os.makedirs(out, exist_ok=True)


def png(path, w, h, pixel):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            raw.extend(pixel(x, y))
    def chunk(t, d):
        c = struct.pack('>I', len(d)) + t + d
        return c + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    data = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0))
    data += chunk(b'IDAT', zlib.compress(bytes(raw), 9)) + chunk(b'IEND', b'')
    open(path, 'wb').write(data)


def quad(x, y):
    top, left = y < 32, x < 32
    if top and left: return (255, 0, 0)
    if top: return (0, 255, 0)
    if left: return (0, 0, 255)
    return (255, 255, 255)

png(os.path.join(out, 'basecolor.png'), 64, 64, quad)
png(os.path.join(out, 'mr.png'), 8, 8, lambda x, y: (0, 128, 255))      # G=rough .5, B=metal 1
png(os.path.join(out, 'normal.png'), 8, 8, lambda x, y: (128, 128, 255))

buf = bytearray()
views, accessors = [], []

def add(data, comp, typ, count, target=None, minmax=None):
    while len(buf) % 4: buf.append(0)
    off = len(buf); buf.extend(data)
    v = {'buffer': 0, 'byteOffset': off, 'byteLength': len(data)}
    if target: v['target'] = target
    views.append(v)
    a = {'bufferView': len(views) - 1, 'componentType': comp, 'count': count, 'type': typ}
    if minmax: a['min'], a['max'] = minmax
    accessors.append(a)
    return len(accessors) - 1

F, U16, U8 = 5126, 5123, 5121
f32 = lambda vals: struct.pack('<%df' % len(vals), *vals)

# primitive 0: textured skinned strip
P = [(-.5, 0, 0), (.5, 0, 0), (-.5, 1, 0), (.5, 1, 0), (-.5, 2, 0), (.5, 2, 0)]
UV = [(0, 1), (1, 1), (0, .5), (1, .5), (0, 0), (1, 0)]
COL = [(1, 0, 0, 1), (0, 1, 0, 1), (0, 0, 1, 1), (1, 1, 0, 1), (1, 0, 1, 1), (0, 1, 1, 1)]
J = [(0, 1, 0, 0)] * 6
W = [(1, 0, 0, 0), (1, 0, 0, 0), (.5, .5, 0, 0), (.5, .5, 0, 0), (0, 1, 0, 0), (0, 1, 0, 0)]
IDX = [0, 1, 3, 0, 3, 2, 2, 3, 5, 2, 5, 4]
MT = [(0, 0, 0)] * 4 + [(0, 0, .5)] * 2
flat = lambda l: [c for t in l for c in t]
pos0 = add(f32(flat(P)), F, 'VEC3', 6, 34962, ([-.5, 0, 0], [.5, 2, 0]))
nrm0 = add(f32([0, 0, 1] * 6), F, 'VEC3', 6, 34962)
uv0 = add(f32(flat(UV)), F, 'VEC2', 6, 34962)
col0 = add(f32(flat(COL)), F, 'VEC4', 6, 34962)
jnt0 = add(bytes(flat(J)), U8, 'VEC4', 6, 34962)
wgt0 = add(f32(flat(W)), F, 'VEC4', 6, 34962)
idx0 = add(struct.pack('<12H', *IDX), U16, 'SCALAR', 12, 34963)
mt0 = add(f32(flat(MT)), F, 'VEC3', 6, 34962, ([0, 0, 0], [0, 0, .5]))

# primitive 1: plain red triangle (second material), bound to joint 0
P1 = [(1, 0, 0), (2, 0, 0), (1.5, 1, 0)]
pos1 = add(f32(flat(P1)), F, 'VEC3', 3, 34962, ([1, 0, 0], [2, 1, 0]))
nrm1 = add(f32([0, 0, 1] * 3), F, 'VEC3', 3, 34962)
jnt1 = add(bytes([0, 0, 0, 0] * 3), U8, 'VEC4', 3, 34962)
wgt1 = add(f32([1, 0, 0, 0] * 3), F, 'VEC4', 3, 34962)
idx1 = add(struct.pack('<3H', 0, 1, 2), U16, 'SCALAR', 3, 34963)
mt1 = add(f32([0, 0, 0] * 3), F, 'VEC3', 3, 34962, ([0, 0, 0], [0, 0, 0]))

# skin: joint0 at origin, joint1 at (0,1,0) -> IBM = inverse(world bind)
ident = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
ibm1 = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1]  # column-major T(0,-1,0)
ibm = add(f32(ident + ibm1), F, 'MAT4', 2)

# animation data
s = math.sin(math.radians(22.5)); c = math.cos(math.radians(22.5))
t3 = add(f32([0, 1, 2]), F, 'SCALAR', 3, None, ([0], [2]))
rot = add(f32([0, 0, 0, 1, 0, 0, s, c, 0, 0, 0, 1]), F, 'VEC4', 3)
t2 = add(f32([0, 2]), F, 'SCALAR', 2, None, ([0], [2]))
tra = add(f32([0, 0, 0, 1, 0, 0]), F, 'VEC3', 2)
t2b = add(f32([0, 1]), F, 'SCALAR', 2, None, ([0], [1]))
scl = add(f32([1, 1, 1, 1.2, 1.2, 1.2]), F, 'VEC3', 2)
wts = add(f32([0, 1, 0]), F, 'SCALAR', 3)

open(os.path.join(out, 'test.bin'), 'wb').write(buf)

gltf = {
    'asset': {'version': '2.0', 'generator': 'make_gltf.py'},
    'scene': 0,
    'scenes': [{'nodes': [0]}],
    'nodes': [
        {'name': 'Root', 'children': [1, 2]},
        {'name': 'Strip', 'mesh': 0, 'skin': 0},
        {'name': 'Bone0', 'children': [3]},
        {'name': 'Bone1', 'translation': [0, 1, 0]},
    ],
    'meshes': [{'name': 'StripMesh', 'weights': [0], 'primitives': [
        {'attributes': {'POSITION': pos0, 'NORMAL': nrm0, 'TEXCOORD_0': uv0, 'COLOR_0': col0,
                        'JOINTS_0': jnt0, 'WEIGHTS_0': wgt0},
         'indices': idx0, 'material': 0, 'targets': [{'POSITION': mt0}]},
        {'attributes': {'POSITION': pos1, 'NORMAL': nrm1, 'JOINTS_0': jnt1, 'WEIGHTS_0': wgt1},
         'indices': idx1, 'material': 1, 'targets': [{'POSITION': mt1}]},
    ]}],
    'skins': [{'joints': [2, 3], 'skeleton': 2, 'inverseBindMatrices': ibm}],
    'materials': [
        {'name': 'TexturedMat', 'doubleSided': True,
         'pbrMetallicRoughness': {'baseColorTexture': {'index': 0}, 'baseColorFactor': [1, 1, 1, 1],
                                  'metallicRoughnessTexture': {'index': 1},
                                  'metallicFactor': 1.0, 'roughnessFactor': 1.0},
         'normalTexture': {'index': 2}},
        {'name': 'RedMat', 'pbrMetallicRoughness': {'baseColorFactor': [1, 0, 0, 1],
                                                   'metallicFactor': 0.0, 'roughnessFactor': 0.5}},
    ],
    'textures': [{'source': 0, 'sampler': 0}, {'source': 1, 'sampler': 0}, {'source': 2, 'sampler': 0}],
    'samplers': [{'wrapS': 10497, 'wrapT': 10497}],
    'images': [{'uri': 'basecolor.png'}, {'uri': 'mr.png'}, {'uri': 'normal.png'}],
    'animations': [{'name': 'Anim', 'samplers': [
        {'input': t3, 'output': rot, 'interpolation': 'LINEAR'},
        {'input': t2, 'output': tra, 'interpolation': 'LINEAR'},
        {'input': t2b, 'output': scl, 'interpolation': 'STEP'},
        {'input': t3, 'output': wts, 'interpolation': 'LINEAR'},
    ], 'channels': [
        {'sampler': 0, 'target': {'node': 3, 'path': 'rotation'}},
        {'sampler': 1, 'target': {'node': 0, 'path': 'translation'}},
        {'sampler': 2, 'target': {'node': 2, 'path': 'scale'}},
        {'sampler': 3, 'target': {'node': 1, 'path': 'weights'}},
    ]}],
    'accessors': accessors,
    'bufferViews': views,
    'buffers': [{'uri': 'test.bin', 'byteLength': len(buf)}],
}
json.dump(gltf, open(os.path.join(out, 'test.gltf'), 'w'), indent=1)
print('wrote', out)
