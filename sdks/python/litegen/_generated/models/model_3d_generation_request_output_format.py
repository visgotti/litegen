from enum import Enum


class Model3DGenerationRequestOutputFormat(str, Enum):
    DAE = "dae"
    FBX = "fbx"
    GLB = "glb"
    GLTF = "gltf"
    OBJ = "obj"
    PLY = "ply"
    STL = "stl"
    USDA = "usda"
    USDC = "usdc"
    USDZ = "usdz"
    VALUE_9 = "3mf"

    def __str__(self) -> str:
        return str(self.value)
