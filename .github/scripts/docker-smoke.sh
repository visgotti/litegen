#!/usr/bin/env bash
# docker-smoke.sh <image>: what the Docker image promises, on real containers.
# No vendor is called: provider keys are fake and only register a provider,
# and the one kept file comes from mock/image-gen.
#
#   A  environment variables only, and no database setting: litegen.db and the
#      kept files are in the /app/data volume, through a restart and a new
#      container on the same volume; docker stop ends it cleanly
#   B  a mounted config file is read (/app/litegen.yaml, or the file --config,
#      LITEGEN_CONFIG_FILE_PATH or LITEGEN_CONFIG_FILE names), environment
#      variables beat it, and nothing the image sets outranks it
#   C  runs as uid 1000; refuses a public bind with no master key
#   D  Postgres from the LITEGEN_DATABASE_* parts: nothing in the image
#      outranks them
#
# SMOKE_PORT: the first of six host ports (default 4000). SMOKE_PREFIX: the
# prefix of every container, volume and network it makes (default
# litegen-smoke); it refuses to start if any already exists, and removes its
# own at exit. Bash 3.2 (macOS) and 5 (CI); needs docker, curl and jq.
# Each check prints PASS or FAIL, and a failed one prints the log of the
# container it is about. Exits 1 if any check failed.
# shellcheck disable=SC2329 # the helpers run through check, eventually and the EXIT trap
set -uo pipefail
IMAGE=${1:?usage: docker-smoke.sh <image>}
PORT=${SMOKE_PORT:-4000}
P=${SMOKE_PREFIX:-litegen-smoke}
MASTER=smoke-master-key
FAILED=0
# The container the checks are about: a failed check prints its log.
CURRENT=

pass() { echo "PASS $*"; }
fail() {
  echo "FAIL $*"
  FAILED=1
  if [[ -n $CURRENT ]] && docker container inspect "$CURRENT" > /dev/null 2>&1; then
    echo "  the end of $CURRENT's log:"
    docker logs "$CURRENT" 2>&1 | tail -n 30 | sed 's/^/  | /'
  fi
}
check() { local name=$1; shift; if "$@"; then pass "$name"; else fail "$name"; fi; }
equals() { [[ "$1" == "$2" ]] || { echo "  expected '$2', got '$1'"; return 1; }; }
# matches <text> <glob>
# shellcheck disable=SC2053 # $2 is a pattern
matches() { [[ "$1" == $2 ]] || { echo "  '$1' does not match $2"; return 1; }; }
# eventually <command...>: it succeeds within 10 s.
eventually() { for _ in $(seq 10); do "$@" && return 0; sleep 1; done; return 1; }

if [[ -n $(docker ps -aq --filter "name=^${P}-") ]] || docker volume inspect "${P}-data" > /dev/null 2>&1 \
  || docker network inspect "${P}-net" > /dev/null 2>&1; then
  echo "containers, volumes or networks named ${P}-* exist already; set SMOKE_PREFIX"; exit 2
fi
TMP=$(mktemp -d)
cleanup() {
  local c
  for c in $(docker ps -aq --filter "name=^${P}-"); do docker rm -fv "$c" > /dev/null; done
  docker volume rm -f "${P}-data" > /dev/null 2>&1
  docker network rm "${P}-net" > /dev/null 2>&1
  rm -rf "$TMP"
}
trap cleanup EXIT

# up <container> <port>: it runs, and /health/live answers within 60 s.
up() {
  local name=$1 port=$2
  for _ in $(seq 60); do
    [[ $(docker inspect -f '{{.State.Running}}' "$name" 2> /dev/null) == true ]] || break
    curl -fsS -o /dev/null "http://127.0.0.1:$port/health/live" 2> /dev/null && return 0
    sleep 1
  done
  echo "  $name is not serving on port $port"
  return 1
}
# exit_code <container>: its exit code once it has stopped (30 s at most),
# "running" if it hasn't, "missing" if there is no such container.
exit_code() {
  local running
  for _ in $(seq 30); do
    running=$(docker inspect -f '{{.State.Running}}' "$1" 2> /dev/null) || { echo missing; return 0; }
    [[ $running == false ]] && { docker inspect -f '{{.State.ExitCode}}' "$1"; return 0; }
    sleep 1
  done
  echo running
}
status() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
api() { local port=$1 key=$2 path=$3; shift 3; curl -fsS -H "Authorization: Bearer $key" "$@" "http://127.0.0.1:$port$path"; }
available() { api "$1" "$2" /v1/models | jq -e --arg p "$3" '[.data[] | select(.provider == $p and .is_available)] | length > 0' > /dev/null; }
unavailable() { api "$1" "$2" /v1/models | jq -e --arg p "$3" '[.data[] | select(.provider == $p)] | length > 0 and all(.[]; .is_available | not)' > /dev/null; }
has_key() { api "$1" "$MASTER" /v1/keys | jq -e --arg n "$2" '[.data[] | select(.name == $n)] | length == 1' > /dev/null; }
in_container() { docker exec "$1" sh -c "$2"; }

# ── A: environment variables only ───────────────────────────────────────────
A=${P}-env
start_a() {
  CURRENT=$A
  docker run -d --name "$A" -p "127.0.0.1:$PORT:4000" -v "${P}-data:/app/data" \
    -e "LITEGEN__MASTER_KEY=$MASTER" -e VIDU_API_KEY=not-a-real-key -e LITEGEN__PROVIDERS__MOCK__API_KEY= "$IMAGE" > /dev/null
}
vidu_quote() {
  api "$PORT" "$MASTER" /v1/videos/cost -H 'content-type: application/json' \
    -d '{"model":"vidu/viduq3-turbo","prompt":"a cat","duration_seconds":5}' | jq -e '.total_cost_usd > 0' > /dev/null
}
start_a
if up "$A" "$PORT"; then
  pass "A: starts with environment variables only and no database setting"
  check "A: /v1/models wants the key" equals "$(status "http://127.0.0.1:$PORT/v1/models")" 401
  check "A: VIDU_API_KEY makes Vidu's models available" available "$PORT" "$MASTER" vidu
  check "A: PixVerse, with no key, is not" unavailable "$PORT" "$MASTER" pixverse
  check "A: a Vidu price quote, with no vendor call" vidu_quote
  check "A: the process runs as uid 1000" equals "$(docker exec "$A" id -u)" 1000
  check "A: the database is /app/data/litegen.db" in_container "$A" 'test -f /app/data/litegen.db'
  check "A: nothing is written into /app itself" in_container "$A" 'test ! -e /app/litegen.db'
  check "A: the database belongs to uid 1000" equals "$(docker exec "$A" stat -c %u /app/data/litegen.db)" 1000
  check "A: a key is created" equals "$(status -H "Authorization: Bearer $MASTER" -H 'content-type: application/json' \
    -d '{"name":"smoke-survives"}' "http://127.0.0.1:$PORT/v1/keys")" 201
  img=$(api "$PORT" "$MASTER" /v1/images/generations -H 'content-type: application/json' \
    -d '{"model":"mock/image-gen","prompt":"a cat"}' | jq -r '.id // empty')
  # An empty id would make the find below match every file.
  check "A: a mock image generation answers with its id" matches "$img" 'litegen-img-*'
  kept="find /app/data/assets -path '*${img:-no-generation-id}*' -type f | grep -q ."
  check "A: the image is kept under /app/data/assets" eventually in_container "$A" "$kept"
  docker restart "$A" > /dev/null
  if up "$A" "$PORT"; then
    check "A: the key survives a restart" has_key "$PORT" smoke-survives
  else
    fail "A: comes back after a restart"
  fi
  docker rm -f "$A" > /dev/null
  start_a
  if up "$A" "$PORT"; then
    check "A: the key survives a new container on the same volume" has_key "$PORT" smoke-survives
    check "A: so does the kept image" in_container "$A" "$kept"
    # litegen is PID 1 and stops on SIGTERM; ignored, Docker kills it after 10 s (137).
    docker stop "$A" > /dev/null
    check "A: docker stop ends it cleanly (exit 0)" equals "$(exit_code "$A")" 0
  else
    fail "A: a new container on the same volume starts"
  fi
else
  fail "A: starts with environment variables only and no database setting"
fi
docker rm -fv "$A" > /dev/null 2>&1

# ── B: a mounted config file ────────────────────────────────────────────────
cat > "$TMP/litegen.yaml" << 'YAML'
master_key: "file-master-key"
providers:
  pixverse:
    api_key: "not-a-real-key"
YAML
printf 'master_key: "file-master-key"\ndatabase_url: "sqlite:///app/data/from-file.db?mode=rwc"\n' > "$TMP/db.yaml"
printf 'master_key: "file-master-key"\nserver:\n  port: 4100\n' > "$TMP/port.yaml"
# The container reads them as uid 1000, whatever the umask made them.
chmod 644 "$TMP"/*.yaml
B=${P}-file
# file_case <label> <port> <docker run args...>: starts on the master key from the file.
file_case() {
  local label=$1 port=$2; shift 2
  CURRENT=$B
  docker run -d --name "$B" -p "127.0.0.1:$port:4000" "$@" > /dev/null
  if up "$B" "$port"; then
    pass "$label: starts on the master key from the file"
    check "$label: the file's key opens the API" equals "$(status -H 'Authorization: Bearer file-master-key' "http://127.0.0.1:$port/v1/models")" 200
    check "$label: the file's PixVerse key makes PixVerse available" available "$port" file-master-key pixverse
  else
    fail "$label: starts on the master key from the file"
  fi
  docker rm -fv "$B" > /dev/null 2>&1
}
file_case "B1 /app/litegen.yaml" $((PORT + 1)) -v "$TMP/litegen.yaml:/app/litegen.yaml:ro" "$IMAGE"
file_case "B4 --config" $((PORT + 2)) -v "$TMP/litegen.yaml:/config/litegen.yaml:ro" "$IMAGE" --config /config/litegen.yaml
file_case "B4 LITEGEN_CONFIG_FILE_PATH" $((PORT + 2)) -v "$TMP/litegen.yaml:/config/litegen.yaml:ro" -e LITEGEN_CONFIG_FILE_PATH=/config/litegen.yaml "$IMAGE"
file_case "B4 LITEGEN_CONFIG_FILE" $((PORT + 2)) -v "$TMP/litegen.yaml:/config/litegen.yaml:ro" -e LITEGEN_CONFIG_FILE=/config/litegen.yaml "$IMAGE"

# B2 names its own database, so only the config file decides what it checks.
CURRENT=$B
docker run -d --name "$B" -p "127.0.0.1:$((PORT + 1)):4000" -v "$TMP/litegen.yaml:/app/litegen.yaml:ro" \
  -e LITEGEN__MASTER_KEY=env-master-key -e 'LITEGEN_DATABASE_URL=sqlite:///app/data/b2.db?mode=rwc' "$IMAGE" > /dev/null
if up "$B" $((PORT + 1)); then
  check "B2: the environment's master key wins" equals "$(status -H 'Authorization: Bearer env-master-key' "http://127.0.0.1:$((PORT + 1))/v1/models")" 200
  check "B2: the file's master key no longer opens it" equals "$(status -H 'Authorization: Bearer file-master-key' "http://127.0.0.1:$((PORT + 1))/v1/models")" 401
  check "B2: the rest of the file still applies" available $((PORT + 1)) env-master-key pixverse
else
  fail "B2: starts with the file and an environment master key"
fi
docker rm -fv "$B" > /dev/null 2>&1

docker run -d --name "$B" -p "127.0.0.1:$((PORT + 3)):4000" -v "$TMP/db.yaml:/app/litegen.yaml:ro" "$IMAGE" > /dev/null
if up "$B" $((PORT + 3)); then
  check "B3: the file's database_url beats the image's default" in_container "$B" 'test -f /app/data/from-file.db && test ! -e /app/data/litegen.db'
else
  fail "B3: starts on the file's database_url"
fi
docker rm -fv "$B" > /dev/null 2>&1

docker run -d --name "$B" -p "127.0.0.1:$((PORT + 4)):4100" -v "$TMP/port.yaml:/app/litegen.yaml:ro" "$IMAGE" > /dev/null
check "B5: the file's server.port is honoured (nothing in the image outranks it)" up "$B" $((PORT + 4))
docker rm -fv "$B" > /dev/null 2>&1

# ── C: user and the refusal ─────────────────────────────────────────────────
CURRENT=
check "C1: the image's user is 1000:1000" equals "$(docker image inspect -f '{{.Config.User}}' "$IMAGE")" 1000:1000
C=${P}-nokey
CURRENT=$C
docker run -d --name "$C" "$IMAGE" > /dev/null
check "C2: no master key on 0.0.0.0: exits 1" equals "$(exit_code "$C")" 1
refuses() { docker logs "$C" 2>&1 | grep -q "refusing to start"; }
check "C2: and says why" refuses
docker rm -fv "$C" > /dev/null 2>&1

# ── D: Postgres from parts ──────────────────────────────────────────────────
docker network create "${P}-net" > /dev/null
CURRENT=${P}-pg
docker run -d --name "${P}-pg" --network "${P}-net" -e POSTGRES_PASSWORD=pw -e POSTGRES_DB=litegen postgres:16-alpine > /dev/null
# Over TCP: during its first-start setup the server listens on its socket only.
for _ in $(seq 60); do docker exec "${P}-pg" pg_isready -h 127.0.0.1 -U postgres -d litegen > /dev/null 2>&1 && break; sleep 1; done
D=${P}-pgparts
CURRENT=$D
docker run -d --name "$D" --network "${P}-net" -p "127.0.0.1:$((PORT + 5)):4000" -e "LITEGEN__MASTER_KEY=$MASTER" \
  -e LITEGEN_DATABASE_HOST="${P}-pg" -e LITEGEN_DATABASE_USER=postgres -e LITEGEN_DATABASE_PASSWORD=pw \
  -e LITEGEN_DATABASE_NAME=litegen "$IMAGE" > /dev/null
if up "$D" $((PORT + 5)); then
  check "D: a key is created" equals "$(status -H "Authorization: Bearer $MASTER" -H 'content-type: application/json' \
    -d '{"name":"smoke-pg"}' "http://127.0.0.1:$((PORT + 5))/v1/keys")" 201
  check "D: it is in Postgres" equals "$(docker exec "${P}-pg" psql -U postgres -d litegen -tAc "select count(*) from api_keys where name = 'smoke-pg'")" 1
  check "D: and no SQLite file was made" in_container "$D" 'test ! -e /app/data/litegen.db'
else
  fail "D: starts on Postgres from the LITEGEN_DATABASE_* parts"
fi

echo
if [[ $FAILED == 0 ]]; then echo "docker-smoke: every check passed"; else echo "docker-smoke: some checks FAILED"; fi
exit $FAILED
