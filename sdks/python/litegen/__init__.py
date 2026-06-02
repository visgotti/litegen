"""LiteGen Python SDK.

>>> from litegen import LiteGenClient
>>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
>>> img = client.images.generate(prompt="a cat", model="openai/dall-e-3")
"""
from __future__ import annotations

__version__ = "0.1.0"

from .client import LiteGenClient, VideoJob
from .async_client import AsyncLiteGenClient, AsyncVideoJob
from .errors import (
    LiteGenError,
    LiteGenAPIError,
    LiteGenValidationError,
    LiteGenTimeoutError,
    LiteGenPollingTimeoutError,
)
from .polling import (
    wait_for_video_completion,
    async_wait_for_video_completion,
)


def _reexport_generated() -> list[str]:
    """Pull pydantic models from `_generated.models` to the top level.

    Names that don't exist in the generated package (because the spec
    didn't include them, or codegen failed on a discriminated union) are
    silently skipped so a partial codegen doesn't break the import.
    """
    import importlib

    exported: list[str] = []
    try:
        mod = importlib.import_module("litegen._generated.models")
    except ModuleNotFoundError:
        return exported

    candidates = [
        "ImageGenerationRequest", "ImageGenerationResponse", "ImageResult",
        "VideoGenerationRequest", "VideoGenerationResponse",
        "ReferenceImage", "RefImageKind",
        "GenerationStatus", "MediaType", "CostSource", "RoutingStrategy",
        "ModelInfo", "ModelSchema", "ModelCapabilities", "ModelPricing",
        "ModelCapabilityFlags", "CapabilityModelPricing", "CapabilityMediaType",
        "PromptSpec", "RefInputSpec", "RefRoleSpec", "RefProviderFormatMultipart",
        "ParamSpecBool", "ParamSpecInt", "ParamSpecFloat", "ParamSpecString",
        "ParamSpecSeed", "ParamSpecAspectRatio",
        "SizeSpecFreeform", "SizeSpecEnum",
        "CostEstimate", "UsageInfo",
        "ProxyStats", "RequestLog", "ProviderHealth",
        "ApiKey", "ApiKeyInfo", "ApiKeyListResponse", "ApiKeyCreatedResponse",
        "ErrorResponse", "ErrorDetail",
        "HealthResponse", "LivenessResponse", "CacheStatus",
        "ModelListResponse", "CacheClearedResponse", "RevokeKeyResponse",
        "BaseGenerationRequest",
    ]
    g = globals()
    for name in candidates:
        if hasattr(mod, name):
            g[name] = getattr(mod, name)
            exported.append(name)
    return exported


__all__ = [
    "__version__",
    "LiteGenClient",
    "VideoJob",
    "AsyncLiteGenClient",
    "AsyncVideoJob",
    "LiteGenError",
    "LiteGenAPIError",
    "LiteGenValidationError",
    "LiteGenTimeoutError",
    "LiteGenPollingTimeoutError",
    "wait_for_video_completion",
    "async_wait_for_video_completion",
] + _reexport_generated()
