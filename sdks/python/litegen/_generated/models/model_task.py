from enum import Enum


class ModelTask(str, Enum):
    ANIMATE = "animate"
    BACKGROUND_SWAP = "background_swap"
    EXTEND = "extend"
    GENERATE = "generate"
    MULTIVIEW = "multiview"
    REMESH = "remesh"
    RETAKE = "retake"
    RIG = "rig"
    SEGMENT = "segment"
    TEXTURE = "texture"
    VIDEO_EDIT = "video_edit"

    def __str__(self) -> str:
        return str(self.value)
