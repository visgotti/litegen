"""Public error types raised by the LiteGen SDK."""
from __future__ import annotations

from typing import Any


class LiteGenError(Exception):
    """Base class for all SDK errors."""


class LiteGenAPIError(LiteGenError):
    """Raised when the API returns a non-2xx response.

    On a validation error, ``param`` names the request field at fault
    (``"output_formats"``, ``"reference_images"``, ``"prompt"``, …) and ``model``
    the model the request named. Branch on ``code`` and ``param`` rather than
    parsing the message.
    """

    def __init__(
        self,
        status: int,
        message: str,
        *,
        type: str = "api_error",
        code: str | None = None,
        provider_error: Any = None,
        param: str | None = None,
        model: str | None = None,
    ) -> None:
        super().__init__(message)
        self.status = status
        self.type = type
        self.code = code
        self.provider_error = provider_error
        self.param = param
        self.model = model


class LiteGenValidationError(LiteGenAPIError):
    """Raised on 400 responses where the server reports `type == "validation_error"`."""


class LiteGenTimeoutError(LiteGenError):
    """Raised when a request times out at the transport layer."""


class LiteGenPollingTimeoutError(LiteGenError):
    """Raised when polling an async job (a video, a 3D generation, …) exceeds its timeout.

    Only the client gave up: the job itself keeps running on the server, and it
    can still complete — and be billed — after this is raised. Resume with the
    family's ``wait_for_completion(job_id)``, or stop it with
    ``client.generations.cancel(job_id)``.
    """

    def __init__(
        self, job_id: str, last_status: str | None = None, kind: str = "video"
    ) -> None:
        # `kind` is what timed out, as it reads in the message: "video",
        # "3D generation", or "job" from the generic poller. It is a trailing
        # argument defaulting to "video" because this error shipped as
        # `(video_id, last_status)`, and those callers must keep producing the
        # exact message they always did.
        suffix = f" (last status: {last_status})" if last_status else ""
        super().__init__(f"Polling for {kind} '{job_id}' timed out{suffix}")
        self.job_id = job_id
        self.kind = kind
        self.last_status = last_status
        # Back-compat alias from when only videos could time out. It holds the
        # id of whatever was polled, 3D generations included.
        self.video_id = job_id
