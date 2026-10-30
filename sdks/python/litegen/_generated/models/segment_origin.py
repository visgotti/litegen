from enum import Enum


class SegmentOrigin(str, Enum):
    GENERATED = "generated"
    SOURCE = "source"

    def __str__(self) -> str:
        return str(self.value)
