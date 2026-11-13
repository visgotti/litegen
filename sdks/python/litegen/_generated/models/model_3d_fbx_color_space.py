from enum import Enum


class Model3DFbxColorSpace(str, Enum):
    LINEAR = "linear"
    SRGB = "srgb"

    def __str__(self) -> str:
        return str(self.value)
