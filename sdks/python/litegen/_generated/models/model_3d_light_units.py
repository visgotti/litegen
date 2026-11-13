from enum import Enum


class Model3DLightUnits(str, Enum):
    BLENDER = "blender"
    PHOTOMETRIC = "photometric"

    def __str__(self) -> str:
        return str(self.value)
