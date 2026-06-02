"""Public error types raised by the LiteGen SDK."""
from __future__ import annotations

from typing import Any


class LiteGenError(Exception):
    """Base class for all SDK errors."""


class LiteGenAPIError(LiteGenError):
    """Raised when the API returns a non-2xx response."""

    def __init__(
        self,
        status: int,
        message: str,
        *,
        type: str = "api_error",
        code: str | None = None,
        provider_error: Any = None,
    ) -> None:
        super().__init__(message)
        self.status = status
        self.type = type
        self.code = code
        self.provider_error = provider_error


class LiteGenValidationError(LiteGenAPIError):
    """Raised on 400 responses where the server reports `type == "validation_error"`."""


class LiteGenTimeoutError(LiteGenError):
    """Raised when a request times out at the transport layer."""


class LiteGenPollingTimeoutError(LiteGenError):
    """Raised when `wait_for_video_completion` exceeds its timeout."""

    def __init__(self, video_id: str, last_status: str | None = None) -> None:
        suffix = f" (last status: {last_status})" if last_status else ""
        super().__init__(f"Polling for video '{video_id}' timed out{suffix}")
        self.video_id = video_id
        self.last_status = last_status
