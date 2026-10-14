from enum import Enum


class Model3DSymmetry(str, Enum):
    AUTO = "auto"
    OFF = "off"
    ON = "on"

    def __str__(self) -> str:
        return str(self.value)
