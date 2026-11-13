#!/usr/bin/env bash
# serve.sh <name> <port> [VAR=value]...: boot a litegen binary in the
# background on 127.0.0.1:<port>, in a directory of its own with a fresh
# SQLite database, the repository's models/ and the mock provider, with the
# given variables in its environment; then wait until it answers
# /health/live.
#
# The binary is $LITEGEN_BINARY, else $RUNNER_TEMP/litegen/litegen. Its log is
# $RUNNER_TEMP/litegen-<name>.log. The server outlives the step, until the job
# ends.
set -euo pipefail
name=${1:?usage: serve.sh <name> <port> [VAR=value]...}
port=${2:?usage: serve.sh <name> <port> [VAR=value]...}
shift 2
root=$(git rev-parse --show-toplevel)
tmp=${RUNNER_TEMP:?serve.sh needs RUNNER_TEMP}
bin=${LITEGEN_BINARY:-$tmp/litegen/litegen}
dir=$(mktemp -d "$tmp/litegen-$name.XXXXXX")
cat > "$dir/litegen.yaml" <<EOF
server:
  host: "127.0.0.1"
  port: $port
database_url: "sqlite://$dir/litegen.db?mode=rwc"
providers:
  mock: {}
EOF
log=$tmp/litegen-$name.log
# The whole background group's stdio goes to the log, and the subshell
# becomes the server (exec): nothing left running holds this step's output
# open, which would keep the step from ever finishing.
(cd "$dir" && exec env LITEGEN_MODELS_DIR="$root/models" "$@" "$bin") < /dev/null > "$log" 2>&1 &
for _ in $(seq 60); do
  if curl -fs -o /dev/null "http://127.0.0.1:$port/health/live"; then
    echo "litegen ($name) answers on 127.0.0.1:$port"
    exit 0
  fi
  sleep 1
done
echo "::error::litegen ($name) never answered on 127.0.0.1:$port"
tail -n 50 "$log"
exit 1
