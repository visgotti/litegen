#!/usr/bin/env bash
# Deprecated shim — folded into deploy.js. Use: node deploy.js web
exec node "$(dirname "$0")/../deploy.js" web "$@"
