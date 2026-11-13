#!/usr/bin/env bash
# Fetch the OpenAPI spec from litegen-core into sdks/openapi.json.
# If LITEGEN_BASE_URL is set, hits that. Otherwise boots a local instance on
# 127.0.0.1:${LITEGEN_CODEGEN_PORT:-4099}.
#
# sdks/openapi.json is replaced only by a fetched OpenAPI 3 document with
# paths: a failed fetch, an HTML page or any other JSON leaves it as it was
# and exits non-zero, saying why. A local instance that exits before it
# answers fails at once, with the end of its own output.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUTPUT="${REPO_ROOT}/sdks/openapi.json"
PORT="${LITEGEN_CODEGEN_PORT:-4099}"

# One throwaway folder for the download, the config, the database and the
# server's log; one trap removes it (and stops the server, if one was started).
WORK="$(mktemp -d)"
SERVER_PID=""
cleanup() {
  if [[ -n "${SERVER_PID}" ]]; then kill "${SERVER_PID}" 2>/dev/null || true; fi
  rm -f "${OUTPUT}.tmp.$$"
  rm -rf "${WORK}"
}
trap cleanup EXIT

# fetch_spec <base-url>: write the spec to OUTPUT, formatted by python3 -m
# json.tool (the committed file's format), only once it checks out.
fetch_spec() {
  local url="$1/openapi.json"
  if ! curl -sS --fail --max-time 120 "${url}" -o "${WORK}/raw.json"; then
    echo "error: could not fetch ${url}; ${OUTPUT} is left as it was" >&2
    return 1
  fi
  if ! python3 -m json.tool "${WORK}/raw.json" > "${WORK}/openapi.json"; then
    echo "error: ${url} did not answer JSON; ${OUTPUT} is left as it was" >&2
    return 1
  fi
  if ! python3 -c '
import json, sys
doc = json.load(open(sys.argv[1]))
ok = isinstance(doc, dict) and str(doc.get("openapi", "")).startswith("3.") and isinstance(doc.get("paths"), dict) and doc["paths"]
sys.exit(0 if ok else 1)' "${WORK}/openapi.json"; then
    echo "error: ${url} answered JSON that is not an OpenAPI 3 document with paths; ${OUTPUT} is left as it was" >&2
    return 1
  fi
  # Same directory, so the rename is atomic. errexit is off inside a function
  # called from `||`, so a failed copy must stop here by hand.
  cp "${WORK}/openapi.json" "${OUTPUT}.tmp.$$" && mv "${OUTPUT}.tmp.$$" "${OUTPUT}" || {
    echo "error: could not write ${OUTPUT}; it is left as it was" >&2
    return 1
  }
  echo "Wrote ${OUTPUT}"
}

if [[ -n "${LITEGEN_BASE_URL:-}" ]]; then
  echo "==> Using running litegen-core at ${LITEGEN_BASE_URL}"
  fetch_spec "${LITEGEN_BASE_URL}"
  exit 0
fi

# The spec comes from the binary booted below, so build it first: a debug
# binary older than the source would hand out a stale spec without a word.
echo "==> Building litegen-core..."
(cd "${REPO_ROOT}/litegen-core" && CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}" cargo build --quiet --bin litegen)

if curl -sf "http://127.0.0.1:${PORT}/health/live" >/dev/null 2>&1; then
  echo "error: something already answers on 127.0.0.1:${PORT}, and its spec could be stale;" >&2
  echo "       stop it, set LITEGEN_CODEGEN_PORT to a free port, or point LITEGEN_BASE_URL at it on purpose" >&2
  exit 1
fi

echo "==> Starting litegen-core for codegen..."

# Use a throwaway sqlite DB and the repo's models/ dir.
cat > "${WORK}/litegen.yaml" <<EOF
server:
  host: "127.0.0.1"
  port: ${PORT}
database_url: "sqlite:///${WORK}/codegen.db?mode=rwc"
EOF

# As .github/scripts/serve.sh does: the background subshell becomes the server
# (exec), so SERVER_PID is the server itself, a child of this shell, which
# reaps it the moment it exits. Its temp files stay in WORK too.
(cd "${WORK}" && exec env TMPDIR="${WORK}" LITEGEN_MODELS_DIR="${REPO_ROOT}/models" "${REPO_ROOT}/litegen-core/target/debug/litegen") \
  < /dev/null > "${WORK}/server.log" 2>&1 &
SERVER_PID=$!

server_failed() {
  echo "error: $1; ${OUTPUT} is left as it was. The server's output:" >&2
  tail -n 40 "${WORK}/server.log" >&2 || true
  exit 1
}

# Wait for liveness on the configured port; stop at once if the server exits.
for _ in $(seq 1 30); do
  if curl -sf "http://127.0.0.1:${PORT}/health/live" >/dev/null 2>&1; then
    fetch_spec "http://127.0.0.1:${PORT}" || server_failed "the spec fetch failed"
    exit 0
  fi
  kill -0 "${SERVER_PID}" 2>/dev/null || server_failed "litegen exited before it answered /health/live"
  sleep 1
done
server_failed "litegen did not answer /health/live on 127.0.0.1:${PORT} within 30 s"
