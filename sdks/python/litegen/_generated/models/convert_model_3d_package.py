from enum import Enum


class ConvertModel3DPackage(str, Enum):
    FILE = "file"
    ZIP = "zip"

    def __str__(self) -> str:
        return str(self.value)
