#!/usr/bin/env bash
# no-skips.sh <log>: fail when a test run's output shows a suite that
# skipped itself.
#
# litegen's environment-gated tests pass, silently, when what they need is
# missing: the Postgres suites without LITEGEN_PG_TEST_URL, the Redis suites
# (shared limits, slots, cache, events and tests/scaling_replicas.rs) without
# LITEGEN_REDIS_TEST_URL, the real-bucket tests without
# LITEGEN_S3_TEST_ENDPOINT, the hook suites without python3, node or bun,
# model3d_chain without scripts/mesh-verify's packages. CI provides all of
# them, so any skip here means CI stopped testing something.
# The log must come from `cargo test -- --show-output` (the messages are in
# the captured output), `pytest -rs` (the skip summary) or `node --test`.
set -euo pipefail
log=${1:?usage: no-skips.sh <log>}
[[ -s $log ]] || { echo "::error::no-skips: $log is missing or empty"; exit 1; }

# cargo test: "SKIPPED: bun not installed", "skipping ...: set LITEGEN_PG_TEST_URL",
#   "skipping: set LITEGEN_S3_TEST_ENDPOINT ...", "skipped the @gltf-transform/core read ...".
# pytest -rs: "SKIPPED [1] tests/conftest.py:43: litegen binary not built ...".
# node --test: "ℹ skipped 1" in the summary (the test's line ends "# <reason>").
# The live suites' form, "skip: missing env ...", in case one stops being #[ignore]d.
pattern='SKIPPED: |^skipping|skipped the |^SKIPPED \[|^ℹ skipped [1-9]|^skip: '
if grep -nE "$pattern" "$log"; then
  echo "::error::a test skipped itself for a missing service or runtime (lines above): CI must provide it"
  exit 1
fi
# A log with no summary at all proves nothing: no test ran, or it was cut
# off. cargo test prints "test result:", node --test "ℹ tests N", pytest "N passed".
if ! grep -qE '^test result: |^ℹ tests [1-9]|[0-9]+ passed' "$log"; then
  echo "::error::no-skips: $log holds no test summary: no test ran, or the log was cut off"
  exit 1
fi
echo "no-skips: nothing skipped itself in $log"
