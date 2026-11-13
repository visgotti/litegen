from enum import Enum


class OwnerScope(str, Enum):
    APP = "app"
    KEY = "key"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
