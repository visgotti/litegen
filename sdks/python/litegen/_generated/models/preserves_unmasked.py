from enum import Enum


class PreservesUnmasked(str, Enum):
    EXACT = "exact"
    SIMILAR = "similar"
    UNSPECIFIED = "unspecified"

    def __str__(self) -> str:
        return str(self.value)
