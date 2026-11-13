from enum import Enum


class Model3DFormatSource(str, Enum):
    GLB = "glb"
    NATIVE = "native"

    def __str__(self) -> str:
        return str(self.value)
