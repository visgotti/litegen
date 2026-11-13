import json
import struct
from pathlib import Path

import pytest

TESTDATA = Path(__file__).resolve().parents[4] / "litegen-core" / "src" / "mesh" / "testdata"


def read_testdata(path: str) -> bytes:
    return (TESTDATA / path).read_bytes()


@pytest.fixture(scope="session")
def skinned_glb() -> bytes:
    return read_testdata("fbx/skinned_small_source.glb")


HUMANOID_MOCK = Path(__file__).resolve().parents[2] / "testdata" / "humanoid_mock.gltf"


def instanced_glb(instances: int, triangles: int = 20_000) -> bytes:
    """A GLB of one mesh of `triangles` unconnected triangles, placed by
    `instances` nodes: instances x 3 x triangles vertices once every instance
    is placed, which is what mesh's measuring (inspect) flattens."""
    n = triangles * 3
    pos = bytearray()
    for k in range(triangles):
        x, y = (k % 200) * 0.01, (k // 200) * 0.01
        pos += struct.pack("<9f", x, y, 0, x + 0.005, y, 0, x, y + 0.005, 0)
    idx = struct.pack(f"<{n}I", *range(n))
    binary = bytes(pos) + idx
    doc = {
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": list(range(instances))}],
        "nodes": [{"mesh": 0, "translation": [i * 3, 0, 0]} for i in range(instances)],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": n, "type": "VEC3", "min": [0, 0, 0], "max": [2, 1.005, 0]},
            {"bufferView": 1, "componentType": 5125, "count": n, "type": "SCALAR"},
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": len(pos)},
            {"buffer": 0, "byteOffset": len(pos), "byteLength": len(idx)},
        ],
        "buffers": [{"byteLength": len(binary)}],
    }
    text = json.dumps(doc).encode()
    text += b" " * (-len(text) % 4)
    total = 12 + 8 + len(text) + 8 + len(binary)
    return (
        b"glTF" + struct.pack("<II", 2, total)
        + struct.pack("<I", len(text)) + b"JSON" + text
        + struct.pack("<I", len(binary)) + b"BIN\0" + binary
    )
