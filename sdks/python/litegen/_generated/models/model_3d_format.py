from enum import Enum


class Model3DFormat(str, Enum):
    DAE = "dae"
    FBX = "fbx"
    GLB = "glb"
    GLTF = "gltf"
    OBJ = "obj"
    PLY = "ply"
    STL = "stl"
    USDA = "usda"
    USDZ = "usdz"
    VALUE_8 = "3mf"

    def __str__(self) -> str:
        return str(self.value)
