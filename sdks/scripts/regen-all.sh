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
# Use --meta none so the tool only emits the package directory (no pyproject.toml,
# README, or .gitignore) and writes it to a throwaway temp location. We then move
# just the package contents into litegen/_generated/, leaving our hand-written
# package files untouched.
if command -v pipx >/dev/null 2>&1; then
  pipx run openapi-python-client generate \
    --path "${SDKS_DIR}/openapi.json" \
    --config "${SDKS_DIR}/python/codegen.yml" \
    --meta none \
    --output-path "${PYGEN_TMP}/_generated" || true
else
  python3 -m pip install --quiet --user openapi-python-client
  python3 -m openapi_python_client generate \
    --path "${SDKS_DIR}/openapi.json" \
    --config "${SDKS_DIR}/python/codegen.yml" \
    --meta none \
    --output-path "${PYGEN_TMP}/_generated" || true
fi
if [[ -d "${PYGEN_TMP}/_generated" && -f "${PYGEN_TMP}/_generated/__init__.py" ]]; then
  rm -rf "${GEN_DIR}"
  mv "${PYGEN_TMP}/_generated" "${GEN_DIR}"
else
  echo "ERROR: openapi-python-client did not produce expected output at ${PYGEN_TMP}/_generated" >&2
  exit 1
fi

echo "==> Done. Review changes under sdks/ and commit."
