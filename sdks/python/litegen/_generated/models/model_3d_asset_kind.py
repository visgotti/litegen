from enum import Enum


class Model3DAssetKind(str, Enum):
    MESH = "mesh"
    PREVIEW = "preview"
    TEXTURE = "texture"

    def __str__(self) -> str:
        return str(self.value)
