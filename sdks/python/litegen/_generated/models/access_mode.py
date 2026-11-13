from enum import Enum


class AccessMode(str, Enum):
    PRIVATE = "private"
    PUBLIC = "public"
    SIGNED = "signed"

    def __str__(self) -> str:
        return str(self.value)
