from enum import Enum


class CapabilityMediaType(str, Enum):
    IMAGE = "image"
    MODEL3D = "model3d"
    VIDEO = "video"

    def __str__(self) -> str:
        return str(self.value)
