from enum import Enum


class KeyframeTiming(str, Enum):
    ANY = "any"
    EVEN = "even"

    def __str__(self) -> str:
        return str(self.value)
