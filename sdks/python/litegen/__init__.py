"""LiteGen Python SDK.

>>> from litegen import LiteGenClient
>>> client = LiteGenClient(api_key="lg-...", base_url="http://localhost:4000")
>>> img = client.images.generate(prompt="a cat", model="openai/gpt-image-2")
"""
from __future__ import annotations

__version__ = "0.1.0"

from .client import LiteGenClient, Model3dJob, VideoJob
from .async_client import AsyncLiteGenClient, AsyncModel3dJob, AsyncVideoJob
from .errors import (
    LiteGenError,
    LiteGenAPIError,
    LiteGenValidationError,
    LiteGenTimeoutError,
    LiteGenPollingTimeoutError,
)
from .polling import (
    MODEL3D_DEFAULT_TIMEOUT,
    apoll_job,
    apoll_model3d,
    async_wait_for_job,
    async_wait_for_model3d_completion,
    async_wait_for_video_completion,
    poll_job,
    poll_model3d,
    wait_for_job,
    wait_for_model3d_completion,
    wait_for_video_completion,
)
# The 3D request/response shapes are static TypedDicts (plain dicts at runtime).
# The rest of litegen.types (ReferenceImage, CostEstimate, UsageInfo,
# GenerationStatus, …) stays in that module: those names at this level already
# belong to the generated classes re-exported below.
from .types import (
    Model3dAsset,
    Model3dConversion,
    Model3dConvertFormats,
    Model3dGenerateParams,
    Model3dGenerationResponse,
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
        # The generated 3D classes (Model3DGenerationRequest,
        # Model3DGenerationResponse, Model3DAsset, Model3DAssetKind) are left
        # out on purpose. This level exports the `litegen.types` TypedDicts
        # Model3dGenerationResponse / Model3dAsset, which describe the dicts the
        # client actually returns; the attrs classes would sit beside them as
        # different types whose names differ by one letter's case.
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
    "Model3dJob",
    "AsyncLiteGenClient",
    "AsyncVideoJob",
    "AsyncModel3dJob",
    "LiteGenError",
    "LiteGenAPIError",
    "LiteGenValidationError",
    "LiteGenTimeoutError",
    "LiteGenPollingTimeoutError",
    "wait_for_video_completion",
    "async_wait_for_video_completion",
    "poll_job",
    "apoll_job",
    "wait_for_job",
    "async_wait_for_job",
    "poll_model3d",
    "apoll_model3d",
    "wait_for_model3d_completion",
    "async_wait_for_model3d_completion",
    "MODEL3D_DEFAULT_TIMEOUT",
    "Model3dGenerateParams",
    "Model3dGenerationResponse",
    "Model3dAsset",
    "Model3dConversion",
    "Model3dConvertFormats",
] + _reexport_generated()
