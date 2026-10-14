#!/bin/bash
# usage: chk.sh [usdchecker args...] file  -> filtered output + real exit code
out=$(usdchecker "$@" 2>&1); rc=$?
printf '%s\n' "$out" | grep -v -E "Coding Error: in RegisterBehaviorForPrimTypeId|^$"
echo "[usdchecker exit=$rc]"
