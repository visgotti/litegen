from enum import Enum


class Model3DDaeMorphs(str, Enum):
    BAKE = "bake"
    KEEP = "keep"

    def __str__(self) -> str:
        return str(self.value)
