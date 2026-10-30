from enum import Enum


class SegmentationPromptType(str, Enum):
    BOX = "box"
    POINT = "point"
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
