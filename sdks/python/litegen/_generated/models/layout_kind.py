from enum import Enum


class LayoutKind(str, Enum):
    OBJECT = "object"
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
