from enum import Enum


class Model3DJointNaming(str, Enum):
    MIXAMO = "mixamo"
    UNITY = "unity"
    UNREAL = "unreal"
    VENDOR = "vendor"
    VRM = "vrm"

    def __str__(self) -> str:
        return str(self.value)
