from enum import Enum


class SegmentationPromptType0Type(str, Enum):
    POINT = "point"

    def __str__(self) -> str:
        return str(self.value)
