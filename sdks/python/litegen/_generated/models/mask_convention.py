from enum import Enum


class MaskConvention(str, Enum):
    BLACK_EDITS = "black_edits"
    TRANSPARENT_EDITS = "transparent_edits"
    WHITE_EDITS = "white_edits"

    def __str__(self) -> str:
        return str(self.value)
