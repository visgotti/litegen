from enum import Enum


class SegmentationPromptType1Type(str, Enum):
    BOX = "box"

    def __str__(self) -> str:
        return str(self.value)
