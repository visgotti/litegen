from enum import Enum


class ModelSource(str, Enum):
    CATALOG = "catalog"
    RUNTIME = "runtime"

    def __str__(self) -> str:
        return str(self.value)
