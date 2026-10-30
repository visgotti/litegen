from enum import Enum


class ImageOutputFormat(str, Enum):
    JPEG = "jpeg"
    PNG = "png"
    WEBP = "webp"

    def __str__(self) -> str:
        return str(self.value)
