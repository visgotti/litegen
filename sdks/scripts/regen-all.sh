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
# WARNING — the generator version is NOT pinned, and it should be.
#
# Whatever pip resolves rewrites all ~227 generated files in that release's
# style: 0.29.x moves typing.Dict -> dict throughout, which reads as ~4,700
# lines of API change and is not one. The committed output was produced by a
# 0.21.x (see the oneOf+allOf note in sdks/contract-allowlist.json), but 0.21.7
# will not run on Python 3.12+ toolchains — it dies inside click with
# "Secondary flag is not valid for non-boolean flag" — so pinning it here would
# just break the script for everyone on a current Python.
#
# The real fix is to settle on a modern version, regenerate once in its own
# commit, and pin THAT. Until then: expect churn, and review it before
# committing rather than assuming it came from your schema change.
export PYGEN_VERSION="${PYGEN_VERSION:-}"
PYGEN_SPEC="openapi-python-client${PYGEN_VERSION:+==${PYGEN_VERSION}}"

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
"${PYGEN_TMP}/venv/bin/pip" install --quiet "${PYGEN_SPEC}"
"${PYGEN_TMP}/venv/bin/openapi-python-client" generate \
  --path "${SDKS_DIR}/openapi.json" \
  --config "${SDKS_DIR}/python/codegen.yml" \
  --meta none \
  --output-path "${PYGEN_TMP}/_generated"
if [[ -d "${PYGEN_TMP}/_generated" && -f "${PYGEN_TMP}/_generated/__init__.py" ]]; then
  rm -rf "${GEN_DIR}"
  mv "${PYGEN_TMP}/_generated" "${GEN_DIR}"
else
  echo "ERROR: openapi-python-client did not produce expected output at ${PYGEN_TMP}/_generated" >&2
  exit 1
fi

echo "==> Done. Review changes under sdks/ and commit."
