from enum import Enum


class SignedLinks(str, Enum):
    LITEGEN = "litegen"
    STORE = "store"

    def __str__(self) -> str:
        return str(self.value)
