#!/usr/bin/env bash
# unchanged.sh <path>...: fail when regenerating left any of these paths
# different from the commit, or added a file to them. Run it right after a
# generator, on a clean checkout: a difference means a committed generated
# file is out of date with its source.
set -euo pipefail
(( $# )) || { echo "usage: unchanged.sh <path>..."; exit 2; }
cd "$(git rev-parse --show-toplevel)"
changes=$(git status --porcelain --untracked-files=all -- "$@")
if [[ -n $changes ]]; then
  echo "::error::generated files are out of date with their source; regenerate them and commit:"
  echo "$changes"
  git --no-pager diff --stat -- "$@"
  git --no-pager diff -- "$@" | head -n 200
  exit 1
fi
echo "unchanged: $* match the commit"
