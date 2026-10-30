from enum import Enum


class ImageView(str, Enum):
    BACK = "back"
    BACK_LEFT = "back-left"
    BACK_RIGHT = "back-right"
    BOTTOM = "bottom"
    FRONT = "front"
    FRONT_LEFT = "front-left"
    FRONT_RIGHT = "front-right"
    LEFT = "left"
    RIGHT = "right"
    TOP = "top"

    def __str__(self) -> str:
        return str(self.value)
