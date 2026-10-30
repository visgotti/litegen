from enum import Enum


class Model3DSkeleton(str, Enum):
    AQUATIC = "aquatic"
    AVIAN = "avian"
    HEXAPOD = "hexapod"
    HUMANOID = "humanoid"
    OCTOPOD = "octopod"
    OTHER = "other"
    QUADRUPED = "quadruped"
    SERPENTINE = "serpentine"

    def __str__(self) -> str:
        return str(self.value)
