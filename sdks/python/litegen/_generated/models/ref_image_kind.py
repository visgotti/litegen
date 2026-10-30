from enum import Enum


class RefImageKind(str, Enum):
    BASE64 = "base64"
    BLOB = "blob"
    GENERATION = "generation"
    URL = "url"

    def __str__(self) -> str:
        return str(self.value)
