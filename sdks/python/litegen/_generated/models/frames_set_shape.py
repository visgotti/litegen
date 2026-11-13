from enum import Enum


class FramesSetShape(str, Enum):
    ALWAYS = "always"
    WITHOUT_REFERENCES = "without_references"

    def __str__(self) -> str:
        return str(self.value)
