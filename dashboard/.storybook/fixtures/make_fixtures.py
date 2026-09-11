#!/usr/bin/env python3
"""Generate the Storybook 3D fixtures. Standard library only.

    python3 dashboard/.storybook/fixtures/make_fixtures.py

Writes, next to this script (served at /fixtures/ via `staticDirs`):

  texture.png    256x256 checker, the cube's base-colour map
  preview.png    256x256 isometric render stand-in (a generation's `preview` asset)
  image.png      256x256 sunset, an image generation's output (image/video stories)
  cube.glb       1 m textured cube: 24 vertices, 12 triangles, embedded texture.png
  icosphere.glb  1 m icosphere, 4 subdivisions: 2,562 vertices, 5,120 triangles
  corrupt.glb    a valid GLB header whose JSON chunk is truncated garbage

The output is deterministic (fixed geometry, fixed zlib level), so re-running
this reproduces the committed bytes. The sizes and polycounts it prints are
the ones `src/story-fixtures.ts` declares on its asset records.

Why generate rather than copy binaries in: the stories must render real,
loadable meshes, and a script is reviewable and reproducible where an opaque
binary from elsewhere is neither.
"""

import json
import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parent

# ─── PNG ────────────────────────────────────────────────────────────────────


def png(width, height, pixel):
    """Encode an 8-bit RGB PNG; `pixel(x, y)` returns an (r, g, b) tuple."""
    rows = bytearray()
    for y in range(height):
        rows.append(0)  # filter type: None
        for x in range(width):
            rows.extend(pixel(x, y))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)  # 8-bit, truecolour
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(rows), 9))
        + chunk(b"IEND", b"")
    )


def hex_rgb(h):
    return tuple(int(h[i : i + 2], 16) for i in (1, 3, 5))


def checker_png():
    a, b = hex_rgb("#58a6ff"), hex_rgb("#e1e4e8")
    cell = 32

    def pixel(x, y):
        return a if ((x // cell) + (y // cell)) % 2 == 0 else b

    return png(256, 256, pixel)


def point_in_poly(px, py, poly):
    inside = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]
        xj, yj = poly[j]
        if (yi > py) != (yj > py) and px < (xj - xi) * (py - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def preview_png():
    """An isometric cube on the dashboard ground — stands in for a provider's
    2D preview render, so it reads as "the same object" as cube.glb."""
    size, cx, cy, r = 256, 128, 132, 84
    h = r * math.sqrt(3) / 2
    top = [(cx, cy - r), (cx + h, cy - r / 2), (cx, cy), (cx - h, cy - r / 2)]
    left = [(cx - h, cy - r / 2), (cx, cy), (cx, cy + r), (cx - h, cy + r / 2)]
    right = [(cx, cy), (cx + h, cy - r / 2), (cx + h, cy + r / 2), (cx, cy + r)]
    faces = [(top, hex_rgb("#e1e4e8")), (left, hex_rgb("#58a6ff")), (right, hex_rgb("#1f6feb"))]
    ground = hex_rgb("#0d1117")

    def pixel(x, y):
        for poly, colour in faces:
            if point_in_poly(x + 0.5, y + 0.5, poly):
                return colour
        return ground

    return png(size, size, pixel)


def image_png():
    """A sunset stand-in for an image generation's output, so image stories
    (and the ResultTile contrast story) show a picture, not a 3D asset."""
    size = 256
    sky_top, sky_bottom = hex_rgb("#1b1f4b"), hex_rgb("#f78166")
    # The sea is a distinct navy, not the dashboard ground, so the image's
    # bottom edge stays visible against the page.
    sun, sea = hex_rgb("#ffd33d"), hex_rgb("#1f3a5f")
    horizon = 176

    def pixel(x, y):
        if y >= horizon:
            return sea
        if (x - 128) ** 2 + (y - horizon) ** 2 <= 56 ** 2:
            return sun
        t = y / horizon
        return tuple(round(sky_top[i] + (sky_bottom[i] - sky_top[i]) * t) for i in range(3))

    return png(size, size, pixel)


# ─── GLB ────────────────────────────────────────────────────────────────────

FLOAT, USHORT = 5126, 5123
ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER = 34962, 34963


def pad(data, fill):
    return data + fill * ((4 - len(data) % 4) % 4)


class GlbBuilder:
    """Accumulates bufferViews/accessors into one BIN chunk."""

    def __init__(self):
        self.bin = bytearray()
        self.views = []
        self.accessors = []

    def view(self, data, target=None):
        while len(self.bin) % 4:
            self.bin.append(0)
        v = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target is not None:
            v["target"] = target
        self.bin.extend(data)
        self.views.append(v)
        return len(self.views) - 1

    def floats(self, rows, kind):
        flat = [c for row in rows for c in row]
        view = self.view(struct.pack(f"<{len(flat)}f", *flat), ARRAY_BUFFER)
        acc = {"bufferView": view, "componentType": FLOAT, "count": len(rows), "type": kind}
        if kind == "VEC3":  # POSITION requires min/max; harmless on the others
            acc["min"] = [min(r[i] for r in rows) for i in range(3)]
            acc["max"] = [max(r[i] for r in rows) for i in range(3)]
        self.accessors.append(acc)
        return len(self.accessors) - 1

    def indices(self, idx):
        assert max(idx) < 65536
        view = self.view(struct.pack(f"<{len(idx)}H", *idx), ELEMENT_ARRAY_BUFFER)
        self.accessors.append({"bufferView": view, "componentType": USHORT, "count": len(idx), "type": "SCALAR"})
        return len(self.accessors) - 1

    def encode(self, gltf):
        gltf["asset"] = {"version": "2.0", "generator": "litegen dashboard make_fixtures.py"}
        gltf["buffers"] = [{"byteLength": len(self.bin)}]
        gltf["bufferViews"] = self.views
        gltf["accessors"] = self.accessors
        js = pad(json.dumps(gltf, separators=(",", ":"), sort_keys=True).encode(), b" ")
        bn = pad(bytes(self.bin), b"\x00")
        total = 12 + 8 + len(js) + 8 + len(bn)
        return (
            struct.pack("<III", 0x46546C67, 2, total)
            + struct.pack("<II", len(js), 0x4E4F534A) + js
            + struct.pack("<II", len(bn), 0x004E4942) + bn
        )


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def cube_glb(texture):
    positions, normals, uvs, idx = [], [], [], []
    for n in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]:
        # u ⟂ n, v = n × u, so u × v = n and (0,1,2)(0,2,3) wind CCW seen from outside.
        u = (0, 0, 1) if n[1] != 0 else cross((0, 1, 0), n)
        v = cross(n, u)
        base = len(positions)
        for su, sv, uv in [(-1, -1, (0, 1)), (1, -1, (1, 1)), (1, 1, (1, 0)), (-1, 1, (0, 0))]:
            positions.append(tuple(0.5 * (n[i] + su * u[i] + sv * v[i]) for i in range(3)))
            normals.append(n)
            uvs.append(uv)
        idx += [base, base + 1, base + 2, base, base + 2, base + 3]

    g = GlbBuilder()
    pos = g.floats(positions, "VEC3")
    nor = g.floats(normals, "VEC3")
    tex = g.floats(uvs, "VEC2")
    ind = g.indices(idx)
    image_view = g.view(texture)
    return g.encode({
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "cube"}],
        "meshes": [{"name": "cube", "primitives": [{
            "attributes": {"POSITION": pos, "NORMAL": nor, "TEXCOORD_0": tex},
            "indices": ind, "material": 0,
        }]}],
        "materials": [{"name": "checker", "pbrMetallicRoughness": {
            "baseColorTexture": {"index": 0}, "metallicFactor": 0.0, "roughnessFactor": 0.6,
        }}],
        "textures": [{"source": 0, "sampler": 0}],
        "samplers": [{"magFilter": 9728, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}],
        "images": [{"bufferView": image_view, "mimeType": "image/png"}],
    }), len(idx) // 3


def icosphere_glb(subdivisions=4):
    t = (1 + math.sqrt(5)) / 2
    verts = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t),
             (0, -1, -t), (0, 1, -t), (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    verts = [tuple(c / math.sqrt(sum(x * x for x in p)) for c in p) for p in verts]
    faces = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4),
             (11, 10, 2), (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8),
             (3, 8, 9), (4, 9, 5), (2, 4, 11), (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    for _ in range(subdivisions):
        cache = {}

        def mid(a, b):
            key = (min(a, b), max(a, b))
            if key not in cache:
                p = [(verts[a][i] + verts[b][i]) / 2 for i in range(3)]
                n = math.sqrt(sum(c * c for c in p))
                verts.append(tuple(c / n for c in p))
                cache[key] = len(verts) - 1
            return cache[key]

        nxt = []
        for a, b, c in faces:
            ab, bc, ca = mid(a, b), mid(b, c), mid(c, a)
            nxt += [(a, ab, ca), (b, bc, ab), (c, ca, bc), (ab, bc, ca)]
        faces = nxt

    g = GlbBuilder()
    # Rounded so the float32 bytes do not depend on the last bit of libm.
    pos = g.floats([tuple(round(0.5 * c, 6) for c in v) for v in verts], "VEC3")
    nor = g.floats([tuple(round(c, 6) for c in v) for v in verts], "VEC3")
    ind = g.indices([i for f in faces for i in f])
    return g.encode({
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "icosphere"}],
        "meshes": [{"name": "icosphere", "primitives": [{
            "attributes": {"POSITION": pos, "NORMAL": nor}, "indices": ind, "material": 0,
        }]}],
        "materials": [{"name": "brass", "pbrMetallicRoughness": {
            "baseColorFactor": [0.89, 0.70, 0.25, 1.0], "metallicFactor": 0.6, "roughnessFactor": 0.35,
        }}],
    }), len(faces), len(verts)


def corrupt_glb():
    """Passes the magic/version check, then the JSON chunk is cut off mid-token,
    so the loader fails the way a truncated upload or a bad provider file does."""
    js = b'{"asset":{"version":"2.0"},"scenes":[{"nodes":[0'
    body = struct.pack("<II", 4096, 0x4E4F534A) + js + b"\x00\xff" * 16
    return struct.pack("<III", 0x46546C67, 2, 12 + len(body)) + body


def main():
    texture = checker_png()
    cube, cube_tris = cube_glb(texture)
    ico, ico_tris, ico_verts = icosphere_glb()
    files = {
        "texture.png": texture,
        "preview.png": preview_png(),
        "image.png": image_png(),
        "cube.glb": cube,
        "icosphere.glb": ico,
        "corrupt.glb": corrupt_glb(),
    }
    for name, data in files.items():
        (OUT / name).write_bytes(data)
        print(f"{name:14} {len(data):>8,} bytes")
    print(f"cube.glb       {cube_tris} triangles")
    print(f"icosphere.glb  {ico_tris:,} triangles, {ico_verts:,} vertices")


if __name__ == "__main__":
    main()
