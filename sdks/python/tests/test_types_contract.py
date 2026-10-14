"""Drift guard: the static types in ``litegen.types`` must match ``sdks/openapi.json``.

Those TypedDicts are written by hand (the client returns plain dicts, so the
generated attrs classes don't describe what callers get), and nothing at runtime
would notice if the wire contract moved under them. This test does:

- response shapes: same keys, same required keys, same nullability;
- the 3D request: same key set, minus deliberate exclusions;
- enum aliases: same values as the schema enums.

Purely static: it reads the spec file and the TypedDicts' own metadata
(``__required_keys__``, ``__optional_keys__``, resolved annotations).
"""
from __future__ import annotations

import json
from pathlib import Path
from typing import Any, get_args

import pytest
from typing_extensions import get_type_hints

from litegen import types as t

# sdks/python/tests/test_types_contract.py -> parents[2] == sdks/
SPEC_PATH = Path(__file__).resolve().parents[2] / "openapi.json"
SCHEMAS: dict[str, Any] = json.loads(SPEC_PATH.read_text())["components"]["schemas"]


def _resolve(name: str) -> tuple[dict[str, Any], set[str]]:
    """A schema's properties and required keys, with ``allOf`` composition flattened."""
    schema = SCHEMAS[name]
    props: dict[str, Any] = {}
    required: set[str] = set()
    for part in schema.get("allOf", [schema]):
        if "$ref" in part:
            p, r = _resolve(part["$ref"].rsplit("/", 1)[-1])
        else:
            p, r = part.get("properties", {}), set(part.get("required", []))
        props.update(p)
        required |= r
    return props, required


def _keys(td: Any) -> set[str]:
    return set(td.__required_keys__ | td.__optional_keys__)


def _nullable(prop: dict[str, Any]) -> bool | None:
    """Whether the spec lets this property be ``null``; ``None`` when the property
    is untyped (any JSON value, null included)."""
    if "type" in prop:
        types = prop["type"]
        return "null" in types if isinstance(types, list) else types == "null"
    variants = prop.get("oneOf") or prop.get("anyOf")
    if variants:
        return any(v.get("type") == "null" for v in variants)
    if "$ref" in prop:
        return False
    return None


# Schema name -> the TypedDict that mirrors it exactly.
MIRRORED = {
    "Model3dGenerationResponse": t.Model3dGenerationResponse,
    "Model3dAsset": t.Model3dAsset,
    "UsageInfo": t.UsageInfo,
    "CostEstimate": t.CostEstimate,
    "Generation": t.Generation,
    "Model3dConvertFormats": t.Model3dConvertFormats,
    "ReferenceImage": t.ReferenceImage,
    "PaginatedResponse_Generation": t.GenerationPage,
}


@pytest.mark.parametrize("schema_name", sorted(MIRRORED))
def test_keys_and_required_keys_match_the_schema(schema_name: str) -> None:
    td = MIRRORED[schema_name]
    props, required = _resolve(schema_name)
    assert _keys(td) == set(props), f"{td.__name__} keys drifted from {schema_name}"
    assert set(td.__required_keys__) == required, (
        f"{td.__name__} required keys drifted from {schema_name}"
    )


@pytest.mark.parametrize("schema_name", sorted(MIRRORED))
def test_nullability_matches_the_schema(schema_name: str) -> None:
    td = MIRRORED[schema_name]
    props, _ = _resolve(schema_name)
    hints = get_type_hints(td)
    for key, prop in props.items():
        nullable = _nullable(prop)
        if nullable is None:
            assert hints[key] is Any, f"{td.__name__}.{key} is untyped JSON in the spec: type it Any"
        else:
            accepts_none = type(None) in get_args(hints[key])
            assert accepts_none == nullable, (
                f"{td.__name__}.{key}: the spec says nullable={nullable}"
            )


# `n` is on the request base shared with images and video, but it means nothing
# for 3D: a job always produces exactly one result, and the 3D validator never
# reads it. Model3dGenerateParams leaves it out on purpose. Anything else
# missing is drift.
REQUEST_EXCLUSIONS = {"n"}


def test_model3d_params_cover_the_request_schema() -> None:
    # Key SETS only: required-ness is the server's call and is moving (`prompt`
    # becomes optional on the wire, enforced per model), so it is not compared.
    props, _ = _resolve("Model3dGenerationRequest")
    assert REQUEST_EXCLUSIONS <= set(props), "stale exclusion: the spec dropped it"
    assert _keys(t.Model3dGenerateParams) == set(props) - REQUEST_EXCLUSIONS
    # The SDK's own stance, independent of the spec: only the model is required.
    assert set(t.Model3dGenerateParams.__required_keys__) == {"model"}


@pytest.mark.parametrize(
    ("schema_name", "alias"),
    [
        ("Model3dAssetKind", t.Model3dAssetKind),
        ("GenerationStatus", t.GenerationStatus),
        ("RefImageKind", t.RefImageKind),
        ("CostSource", t.CostSource),
        ("MediaType", t.MediaType),
        # The 3D vocabularies: named enum schemas in the spec, so the spec is
        # the source of truth. Model3dFormat grows as litegen learns to convert
        # new containers locally; regenerating the spec makes this fail until
        # the Literal follows.
        ("Model3dFormat", t.Model3dFormat),
        ("Model3dTopology", t.Model3dTopology),
        ("Model3dSymmetry", t.Model3dSymmetry),
    ],
)
def test_literal_values_match_the_schema_enum(schema_name: str, alias: Any) -> None:
    assert set(get_args(alias)) == set(SCHEMAS[schema_name]["enum"])
