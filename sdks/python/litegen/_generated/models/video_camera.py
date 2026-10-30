from enum import Enum


class VideoCamera(str, Enum):
    AUTO = "auto"
    FIXED = "fixed"

    def __str__(self) -> str:
        return str(self.value)
