from enum import Enum


class EntityType(str, Enum):
    APP = "app"
    END_USER = "end_user"
    KEY = "key"
    MEMBER = "member"
    ORG = "org"

    def __str__(self) -> str:
        return str(self.value)
