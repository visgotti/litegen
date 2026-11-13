from enum import Enum


class Model3DUsdClips(str, Enum):
    ALL = "all"
    FIRST = "first"

    def __str__(self) -> str:
        return str(self.value)
