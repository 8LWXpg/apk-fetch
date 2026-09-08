#!/usr/bin/env bash
#
# Re-fetch the APKPure HTML fixtures that src/parse.rs tests run against.
#
#   bash crates/providers/apkpure/tests/refresh-fixtures.sh
#
# Run this when APKPure changes its markup: check the diff, then adjust the
# selectors in src/parse.rs. The tests assert on structure, not specific version
# numbers, so a refresh rarely breaks them. Fixtures are trimmed of
# <script>/<style>/<svg> to keep the repo small; that removes no structure the
# parsers use. APKMirror has its own script at ../../apkmirror/tests/.

set -euo pipefail

UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36'
BASE='https://apkpure.com'
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/fixtures" && pwd)"

TRIM='import sys,re
h=sys.stdin.read()
for t in ("script","style","svg","noscript"):
    h=re.sub(r"<"+t+r"\b[^>]*>.*?</"+t+r">","",h,flags=re.S|re.I)
sys.stdout.write(h)'

get()  { curl -fsSL -A "$UA" "$1"; }
save() { python -c "$TRIM" > "$DIR/$1"; echo "  $1  ($(wc -c < "$DIR/$1") bytes)"; }

get "$BASE/search?q=firefox"               | save search-firefox.html
get "$BASE/x/org.mozilla.firefox"          | save app-firefox.html
get "$BASE/x/org.mozilla.firefox/versions" | save versions-firefox.html

echo "done — now: cargo test -p apk-fetch-apkpure"
