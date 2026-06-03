"""Contract test: the hand-written facade must cover every OpenAPI operation.

Purely static (no network, no running server):

1. Parse ``sdks/openapi.json`` into a set of ``METHOD /normalized/path``.
2. Scan ``litegen/client.py`` for every ``_request("METHOD", "/path")`` call and
   normalize the same way (strip query string, collapse ``{id}`` -> ``{}``).
3. Diff the two sets, honoring two ratchets in ``sdks/contract-allowlist.json``:
     - ``known_uncovered.python``   — spec ops with no facade method yet
     - ``known_undocumented.paths`` — facade calls for core endpoints the spec omits

The ratchets keep the suite green today despite the Python facade's gaps, but
the test fails the moment a NEW spec operation ships without a method, the
facade calls an undeclared/undocumented path, or a ratchet entry becomes
covered/stale. The TypeScript SDK has the same test reading the same file.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

# sdks/python/tests/test_contract.py -> parents[2] == sdks/
SDKS_ROOT = Path(__file__).resolve().parents[2]
SPEC_PATH = SDKS_ROOT / "openapi.json"
FACADE_PATH = SDKS_ROOT / "python" / "litegen" / "client.py"
ALLOWLIST_PATH = SDKS_ROOT / "contract-allowlist.json"

HTTP_METHODS = ("GET", "POST", "PUT", "PATCH", "DELETE")

# Matches:  _request("POST", "/v1/...")  and  _request("GET", f"/v1/.../{id}").
# The leading-slash requirement avoids matching method arrays etc.
_CALL_RE = re.compile(r'"(GET|POST|PUT|PATCH|DELETE)"\s*,\s*f?"(/[^"]+)"')


def _normalize_path(path: str) -> str:
    """Collapse path params + drop query string so paths compare across sides."""
    path = path.split("?")[0]
    path = re.sub(r"\$\{[^}]+\}", "{}", path)  # belt & suspenders (TS-style)
    path = re.sub(r"\{[^}]+\}", "{}", path)  # f-string / OpenAPI params -> {}
    return path


def _spec_operations() -> set[str]:
    spec = json.loads(SPEC_PATH.read_text())
    ops: set[str] = set()
    for path, item in spec.get("paths", {}).items():
        for method in item:
            if method.upper() in HTTP_METHODS:
                ops.add(f"{method.upper()} {_normalize_path(path)}")
    return ops


def _facade_operations() -> set[str]:
    src = FACADE_PATH.read_text()
    return {
        f"{m.group(1)} {_normalize_path(m.group(2))}" for m in _CALL_RE.finditer(src)
    }


_ALLOW = json.loads(ALLOWLIST_PATH.read_text())
SPEC_OPS = _spec_operations()
FACADE_OPS = _facade_operations()
KNOWN_UNCOVERED = set(_ALLOW.get("known_uncovered", {}).get("python", []))
KNOWN_UNDOCUMENTED = set(_ALLOW.get("known_undocumented", {}).get("paths", []))


def test_every_spec_operation_has_a_method_or_known_gap() -> None:
    uncovered = sorted(
        op for op in SPEC_OPS if op not in FACADE_OPS and op not in KNOWN_UNCOVERED
    )
    # Non-empty => an endpoint shipped without a facade method. Add the method,
    # or (if intentionally unsupported) add it to known_uncovered.python.
    assert uncovered == [], "Spec operations with no SDK method:\n" + "\n".join(uncovered)


def test_no_drift_facade_calls_absent_from_spec() -> None:
    drift = sorted(
        op
        for op in FACADE_OPS
        if op not in SPEC_OPS and op not in KNOWN_UNDOCUMENTED
    )
    # Non-empty => the facade calls a path the spec doesn't declare and that
    # isn't a recorded core-spec gap — likely a typo or renamed endpoint.
    assert drift == [], "Facade calls undeclared paths:\n" + "\n".join(drift)


def test_known_uncovered_has_no_now_covered_or_stale_entries() -> None:
    now_covered = sorted(op for op in KNOWN_UNCOVERED if op in FACADE_OPS)
    assert now_covered == [], "Remove from known_uncovered.python (now covered):\n" + "\n".join(
        now_covered
    )
    stale = sorted(op for op in KNOWN_UNCOVERED if op not in SPEC_OPS)
    assert stale == [], "Stale known_uncovered.python entries (not in spec):\n" + "\n".join(
        stale
    )


def test_known_undocumented_entries_are_used_and_still_absent_from_spec() -> None:
    now_documented = sorted(op for op in KNOWN_UNDOCUMENTED if op in SPEC_OPS)
    assert now_documented == [], "Spec now documents these — remove from known_undocumented:\n" + "\n".join(
        now_documented
    )


def test_spec_and_facade_parsed_non_trivially() -> None:
    # Guards against a locator/parse bug silently turning the contract green.
    assert len(SPEC_OPS) >= 25
    assert len(FACADE_OPS) >= 5
