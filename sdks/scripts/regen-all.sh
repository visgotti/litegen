#!/usr/bin/env bash
# Regenerate the OpenAPI snapshot and both SDKs from it.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
SDKS_DIR="${REPO_ROOT}/sdks"

echo "==> Fetching OpenAPI spec"
"${SCRIPT_DIR}/fetch-openapi.sh"

echo "==> Regenerating TypeScript SDK"
cd "${SDKS_DIR}/typescript"
if [[ ! -d node_modules ]]; then
  npm install --silent --no-audit --no-fund
fi
npx --yes openapi-typescript "${SDKS_DIR}/openapi.json" \
  -o "${SDKS_DIR}/typescript/src/generated/schema.d.ts"

echo "==> Regenerating Python SDK"
GEN_DIR="${SDKS_DIR}/python/litegen/_generated"
PYGEN_TMP="$(mktemp -d)"
trap 'rm -rf "${PYGEN_TMP}"' EXIT
# The generator is pinned to the release that produced the committed output.
#
# An unpinned install rewrites all ~227 generated files in whatever style pip
# resolves: 0.29.x moves typing.Dict -> dict throughout, which reads as ~4,700
# lines of API change and is not one. So 0.21.7 it stays, until someone moves
# to a modern version deliberately, in a commit of its own.
#
# `click` is pinned with it. 0.21.7's typer is incompatible with click >= 8.2,
# which fails at startup with "Secondary flag is not valid for non-boolean
# flag" — that, not the Python version, is what used to break this script on a
# current toolchain. With click 8.1.8 the same output comes out byte-identical
# under Python 3.11 and 3.14 (checked 2026-10-12). `ruff` is installed for the
# post_hooks in codegen.yml.
export PYGEN_VERSION="${PYGEN_VERSION:-0.21.7}"
PYGEN_SPEC="openapi-python-client==${PYGEN_VERSION}"

# Use --meta none so the tool only emits the package directory (no pyproject.toml,
# README, or .gitignore) and writes it to a throwaway temp location. We then move
# just the package contents into litegen/_generated/, leaving our hand-written
# package files untouched.
#
# Always through a throwaway venv: `pip install --user` fails outright on any
# Homebrew or system Python from 3.12 on (PEP 668, externally-managed
# environment). That failure used to be swallowed by a trailing `|| true`, so
# the script printed "Done" and exited 0 having regenerated nothing — the
# Python SDK then drifted silently behind the spec. No `|| true` anywhere here:
# if generation fails, this script fails.
python3 -m venv "${PYGEN_TMP}/venv"
"${PYGEN_TMP}/venv/bin/pip" install --quiet "${PYGEN_SPEC}" "click==8.1.8" ruff
# The venv's bin goes first on PATH so the codegen.yml post_hooks find its ruff.
PATH="${PYGEN_TMP}/venv/bin:${PATH}" "${PYGEN_TMP}/venv/bin/openapi-python-client" generate \
  --path "${SDKS_DIR}/openapi.json" \
  --config "${SDKS_DIR}/python/codegen.yml" \
  --meta none \
  --output-path "${PYGEN_TMP}/_generated"
if [[ -d "${PYGEN_TMP}/_generated" && -f "${PYGEN_TMP}/_generated/__init__.py" ]]; then
  # The ruff post_hooks leave their cache in the output; it is not source.
  rm -rf "${PYGEN_TMP}/_generated/.ruff_cache"
  rm -rf "${GEN_DIR}"
  mv "${PYGEN_TMP}/_generated" "${GEN_DIR}"
else
  echo "ERROR: openapi-python-client did not produce expected output at ${PYGEN_TMP}/_generated" >&2
  exit 1
fi

echo "==> Done. Review changes under sdks/ and commit."
