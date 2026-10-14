from enum import Enum


class Model3DTopology(str, Enum):
    QUAD = "quad"
    TRIANGLE = "triangle"

    def __str__(self) -> str:
        return str(self.value)
