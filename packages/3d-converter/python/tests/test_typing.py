"""The type stub: true to the runtime module (mypy's stubtest), and strict
enough that wrong calls fail type-checking (mypy --strict and pyright strict
over typed_usage.py, whose every `type: ignore` must be needed)."""

import ast
import filecmp
import os
import subprocess
import tempfile
import sys
from pathlib import Path

USAGE = Path(__file__).with_name("typed_usage.py")


def run(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run([sys.executable, "-m", *args], capture_output=True, text=True, check=False)


ALLOWLIST = Path(__file__).with_name("stubtest-allowlist.txt")


def test_stubtest_passes() -> None:
    r = run("mypy.stubtest", "litegen_3d_converter", "--allowlist", str(ALLOWLIST))
    assert r.returncode == 0, r.stdout + r.stderr


def test_typed_usage_passes_mypy_strict() -> None:
    r = run("mypy", "--strict", "--warn-unused-ignores", "--no-incremental", str(USAGE))
    assert r.returncode == 0, r.stdout + r.stderr


def test_typed_usage_passes_pyright_strict() -> None:
    r = run("pyright", str(USAGE))
    assert r.returncode == 0, r.stdout + r.stderr


STUB = Path(__file__).resolve().parents[1] / "litegen_3d_converter.pyi"
STUB_GEN = Path(__file__).resolve().parents[2] / "target" / "debug" / "stub_gen"


SOURCES = [Path(__file__).resolve().parents[1] / "src", Path(__file__).resolve().parents[2] / "core" / "src"]


def test_stub_is_current() -> None:
    """The checked-in stub is what the generator writes now. stub_gen is the
    binary scripts/build.sh builds (cargo run --bin stub_gen) before the
    wheel, so this needs no cargo of its own; a Rust source changed since
    that build means both are stale."""
    assert STUB_GEN.exists(), f"{STUB_GEN} is missing: run python/scripts/build.sh"
    built = STUB_GEN.stat().st_mtime
    newer = [str(p) for d in SOURCES for p in d.rglob("*.rs") if p.stat().st_mtime > built]
    assert newer == [], f"changed since the last build (run python/scripts/build.sh): {newer}"
    with tempfile.TemporaryDirectory() as out:
        # pyo3-stub-gen finds pyproject.toml through the variable cargo run sets.
        env = {**os.environ, "CARGO_MANIFEST_DIR": str(STUB.parent)}
        subprocess.run([str(STUB_GEN), out], check=True, capture_output=True, env=env)
        fresh = Path(out) / STUB.name
        assert filecmp.cmp(fresh, STUB, shallow=False), f"{STUB.name} is stale: run python/scripts/build.sh"


def docstring(node: ast.AST) -> str | None:
    return ast.get_docstring(node) if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)) else None


def test_every_stub_declaration_has_a_docstring() -> None:
    """G2: what an editor shows for every class, property, method, function
    and Literal alias comes from the Rust doc comments."""
    tree = ast.parse(STUB.read_text())
    missing: list[str] = []
    body = tree.body
    for i, node in enumerate(body):
        if isinstance(node, (ast.ClassDef, ast.FunctionDef)):
            if not docstring(node):
                missing.append(node.name)
            if isinstance(node, ast.ClassDef):
                members = node.body
                for j, member in enumerate(members):
                    if isinstance(member, ast.FunctionDef) and not docstring(member):
                        missing.append(f"{node.name}.{member.name}")
                    if isinstance(member, ast.AnnAssign) and isinstance(member.target, ast.Name):
                        following = members[j + 1] if j + 1 < len(members) else None
                        if not (isinstance(following, ast.Expr) and isinstance(following.value, ast.Constant)):
                            missing.append(f"{node.name}.{member.target.id}")
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name) and not node.target.id.startswith("_"):
            following = body[i + 1] if i + 1 < len(body) else None
            if not (isinstance(following, ast.Expr) and isinstance(following.value, ast.Constant)):
                missing.append(node.target.id)
    assert missing == []
