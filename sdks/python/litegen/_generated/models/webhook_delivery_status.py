from enum import Enum


class WebhookDeliveryStatus(str, Enum):
    DELIVERED = "delivered"
    FAILED = "failed"
    PENDING = "pending"

    def __str__(self) -> str:
        return str(self.value)
