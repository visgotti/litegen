from enum import Enum


class Model3DAnimationPreset(str, Enum):
    ATTACK_MELEE = "attack_melee"
    ATTACK_RANGED = "attack_ranged"
    CAST = "cast"
    CLIMB = "climb"
    CROUCH = "crouch"
    DEATH = "death"
    FALL = "fall"
    HURT = "hurt"
    IDLE = "idle"
    JUMP = "jump"
    RUN = "run"
    TURN = "turn"
    WALK = "walk"

    def __str__(self) -> str:
        return str(self.value)
