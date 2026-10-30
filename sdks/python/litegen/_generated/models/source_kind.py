from enum import Enum


class SourceKind(str, Enum):
    BASE64 = "base64"
    GENERATION = "generation"
    URL = "url"

    def __str__(self) -> str:
        return str(self.value)
