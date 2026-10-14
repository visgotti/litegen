from enum import Enum


class SpendGroupBy(str, Enum):
    APP = "app"
    DAY = "day"
    END_USER = "end_user"
    KEY = "key"
    MEDIA_TYPE = "media_type"
    MODEL = "model"
    ORG = "org"
    PROVIDER = "provider"
    STATUS = "status"
    TAG = "tag"
    USER = "user"

    def __str__(self) -> str:
        return str(self.value)
