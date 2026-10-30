from enum import Enum


class ExtendOutput(str, Enum):
    ADDED = "added"
    FULL = "full"

    def __str__(self) -> str:
        return str(self.value)
