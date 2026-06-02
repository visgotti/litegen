from enum import Enum


class CostSource(str, Enum):
    DYNAMIC = "dynamic"
    ESTIMATED = "estimated"

    def __str__(self) -> str:
        return str(self.value)
