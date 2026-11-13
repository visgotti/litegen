from enum import Enum


class Model3DKtx2Textures(str, Enum):
    KEEP = "keep"
    PNG = "png"

    def __str__(self) -> str:
        return str(self.value)
