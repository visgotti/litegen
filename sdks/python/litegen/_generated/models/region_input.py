from enum import Enum


class RegionInput(str, Enum):
    BOX = "box"
    MASK = "mask"

    def __str__(self) -> str:
        return str(self.value)
