from enum import Enum


class SegmentationPromptType2Type(str, Enum):
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
