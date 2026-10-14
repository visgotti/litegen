#!/bin/bash
# usage: probe.sh "<query string>" [screenshot.png]   (each Chrome run gets its own profile; hard 60s cap)
D="${D:-$(cd "$(dirname "$0")/.." && pwd)}"
CH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
URL="http://127.0.0.1:8765/test.html?$1"
run() { # $1 profile suffix, rest = args
  local P=$D/chrome-profile-$1; shift; rm -rf "$P"
  "$CH" --headless=new --user-data-dir="$P" --no-first-run --no-default-browser-check \
     --enable-unsafe-swiftshader --use-angle=swiftshader --virtual-time-budget=10000 "$@" &
  local pid=$!; for i in $(seq 1 60); do kill -0 $pid 2>/dev/null || break; sleep 1; done; kill $pid 2>/dev/null; wait $pid 2>/dev/null
}
if [ -n "$2" ]; then run shot --window-size=480,800 --screenshot="$2" "$URL" >/dev/null 2>&1; fi
run dom --dump-dom "$URL" 2>/dev/null > $D/dom_dump.html
python3 -I -c '
import sys, html, re
s = open(sys.argv[1]).read()
m = re.search(r"<pre id=\"out\">(.*?)</pre>", s, re.S)
print(html.unescape(m.group(1)) if m else "NO OUTPUT: " + s[:500])
' $D/dom_dump.html
