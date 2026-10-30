from enum import Enum


class InPlaceMode(str, Enum):
    LITEGEN = "litegen"
    NATIVE = "native"

    def __str__(self) -> str:
        return str(self.value)
