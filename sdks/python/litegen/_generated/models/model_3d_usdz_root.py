from enum import Enum


class Model3DUsdzRoot(str, Enum):
    USDA = "usda"
    USDC = "usdc"

    def __str__(self) -> str:
        return str(self.value)
