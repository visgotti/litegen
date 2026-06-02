from enum import Enum


class RoutingStrategy(str, Enum):
    FALLBACK = "fallback"
    LOWEST_COST = "lowest_cost"
    LOWEST_LATENCY = "lowest_latency"
    WEIGHTED_ROUND_ROBIN = "weighted_round_robin"

    def __str__(self) -> str:
        return str(self.value)
