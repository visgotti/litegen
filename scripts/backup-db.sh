#!/usr/bin/env bash
# Backup litegen DB. Reads LITEGEN__DATABASE_URL from env.
# Usage: LITEGEN__DATABASE_URL=... ./scripts/backup-db.sh /path/to/backup-dir
set -euo pipefail

DEST="${1:-./backups}"
TS=$(date -u +%Y%m%dT%H%M%SZ)
mkdir -p "$DEST"

if [[ -z "${LITEGEN__DATABASE_URL:-}" ]]; then
  echo "LITEGEN__DATABASE_URL not set" >&2
  exit 1
fi

url="$LITEGEN__DATABASE_URL"
case "$url" in
  sqlite:*|sqlite://*)
    # sqlite path
    path="${url#sqlite://}"; path="${path#sqlite:}"; path="${path%%\?*}"
    [[ -f "$path" ]] || { echo "sqlite file not found: $path" >&2; exit 2; }
    out="$DEST/litegen-${TS}.sqlite"
    # Use the .backup PRAGMA via sqlite3 CLI — safe under concurrent writes
    sqlite3 "$path" ".backup '$out'"
    echo "$out"
    ;;
  postgres://*|postgresql://*)
    out="$DEST/litegen-${TS}.sql.gz"
    pg_dump "$url" | gzip > "$out"
    echo "$out"
    ;;
  *)
    echo "unsupported database url: $url" >&2
    exit 3
    ;;
esac
