from enum import Enum


class ImageBackground(str, Enum):
    OPAQUE = "opaque"
    TRANSPARENT = "transparent"

    def __str__(self) -> str:
        return str(self.value)
