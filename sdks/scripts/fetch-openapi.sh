#!/usr/bin/env bash
# Fetch the OpenAPI spec from litegen-core into sdks/openapi.json.
# If LITEGEN_BASE_URL is set, hits that. Otherwise boots a local instance.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUTPUT="${REPO_ROOT}/sdks/openapi.json"

if [[ -n "${LITEGEN_BASE_URL:-}" ]]; then
  echo "==> Using running litegen-core at ${LITEGEN_BASE_URL}"
  curl -sf "${LITEGEN_BASE_URL}/openapi.json" \
    | python3 -m json.tool > "${OUTPUT}"
  echo "Wrote ${OUTPUT}"
  exit 0
fi

echo "==> Starting litegen-core for codegen..."

# Use a throwaway sqlite DB and the repo's models/ dir.
TMPDIR="$(mktemp -d)"
trap 'rm -rf "${TMPDIR}"' EXIT
cat > "${TMPDIR}/litegen.yaml" <<EOF
server:
  host: "127.0.0.1"
  port: 4099
database_url: "sqlite:///${TMPDIR}/codegen.db?mode=rwc"
EOF

(
  cd "${TMPDIR}"
  LITEGEN_MODELS_DIR="${REPO_ROOT}/models" \
    "${REPO_ROOT}/litegen-core/target/debug/litegen" >/dev/null 2>&1 &
  echo $! > "${TMPDIR}/server.pid"
)
SERVER_PID="$(cat "${TMPDIR}/server.pid")"
trap 'kill "${SERVER_PID}" 2>/dev/null || true; rm -rf "${TMPDIR}"' EXIT

# Wait for liveness on the configured port.
for _ in $(seq 1 30); do
  if curl -sf http://127.0.0.1:4099/health/live >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

curl -sf http://127.0.0.1:4099/openapi.json \
  | python3 -m json.tool > "${OUTPUT}"
echo "Wrote ${OUTPUT}"
